use axum::{
    Json, Router,
    extract::{self, Extension},
    response,
    routing::{get, post},
};
use chrono::DateTime;
use chrono::Local;
use serde::{Deserialize, Serialize};
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::Path;
use std::sync::Mutex;
use std::{
    collections::HashMap,
    process::{Command, Stdio},
    sync::Arc,
};
use tokio::sync::mpsc;
use tokio::time::{Duration, sleep};
use tower_http::trace::TraceLayer;
use tracing;
use tracing_subscriber::EnvFilter;

#[cfg(unix)]
use nix::sys::signal::{Signal, kill};
#[cfg(unix)]
use nix::unistd::Pid;

#[cfg(windows)]
use windows_sys::Win32::System::Threading::{OpenProcess, TerminateProcess};

#[cfg(unix)]
fn kill_process(pid: u32) {
    let _ = kill(Pid::from_raw(pid as i32), Signal::SIGKILL);
}

#[cfg(windows)]
fn kill_process(pid: u32) {
    unsafe {
        let handle = OpenProcess(1, 0, pid); // PROCESS_TERMINATE
        TerminateProcess(handle, 1);
    }
}

#[derive(Debug, Deserialize, Clone)]
pub struct Config {
    pub http: HttpConfig,
    pub processes: Vec<ProcessConfig>,
    pub home :String,
    pub log_dir: String,
}

#[derive(Debug, Deserialize, Clone)]
pub struct HttpConfig {
    pub addr: String,
}

#[derive(Serialize, Debug, Deserialize, Clone)]
pub struct ProcessConfig {
    pub name: String,
    pub cmd: String,
    pub args: Vec<String>,
    pub envs: Vec<String>, // 额外的环境变量值
    pub home: String, // 进程根目录

    pub redirect_output: bool,     // 是否重定向 stdout 和 stderr 到日志
    pub output_dir: String,        // 单独的输出目录
    pub max_run: Option<Duration>, // 最大运行时长，秒数
}

#[derive(Serialize, Clone, Debug)]
pub enum ProcState {
    Ready,
    Starting,
    Running,
    Stopped,
    Exited(i32),
    Backoff,
}

pub struct ProcessEntry {
    pub state: ProcState,
    pub cmd: ProcessConfig,
    pub pid: Option<u32>,
    pub control_tx: mpsc::Sender<ControlMsg>,
    pub start_time: Option<DateTime<Local>>,
    pub start_count: u64,
}
pub enum ControlMsg {
    Kill,
    Restart,
}

pub struct Registry {
    inner: Mutex<HashMap<String, ProcessEntry>>,
}

#[derive(Serialize, Clone, Debug)]
pub struct ProcessOut {
    pub name: String,
    pub cmd: ProcessConfig,
    pub state: ProcState,
    pub pid: u32,
    pub start_time: Option<String>,
    pub start_count: u64,
}

impl Registry {
    pub fn new() -> Self {
        Registry {
            inner: Mutex::new(HashMap::new()),
        }
    }

    pub fn register_process(&self, name: &str, cmd: ProcessConfig, tx: mpsc::Sender<ControlMsg>) {
        let mut registry = self.inner.lock().unwrap();
        registry.insert(
            name.to_string(),
            ProcessEntry {
                state: ProcState::Ready,
                cmd: cmd,
                pid: None,
                control_tx: tx,
                start_time: None,
                start_count: 0,
            },
        );
        tracing::info!("Registered process {}", name);
    }

    pub fn get_control(&self, name: &str) -> Option<tokio::sync::mpsc::Sender<ControlMsg>> {
        self.inner
            .lock()
            .unwrap()
            .get(name)
            .map(|e| e.control_tx.clone())
    }

    pub fn set_state(&self, name: &str, state: ProcState) {
        let mut registry = self.inner.lock().unwrap();
        if let Some(entry) = registry.get_mut(name) {
            entry.state = state;
            tracing::info!("set_state {} ", name);
        } else {
            panic!("set_state {} not found", name);
        }
    }

    pub fn set_running(&self, name: &str, pid: u32) {
        let mut registry = self.inner.lock().unwrap();
        if let Some(entry) = registry.get_mut(name) {
            entry.state = ProcState::Running;
            entry.pid = Some(pid);
            tracing::info!(
                "set_running {} -> ( {:?}, {:?} )",
                name,
                ProcState::Running,
                pid
            );
            entry.start_time = Some(Local::now());
            entry.start_count += 1;
        } else {
            panic!("set_running {} not found", name)
        }
    }

    pub fn list(&self) -> Vec<ProcessOut> {
        let registry = self.inner.lock().unwrap();
        registry
            .iter()
            .map(|(k, v)| {
                let start_time_str = v
                    .start_time
                    .map(|t| t.format("%Y-%m-%d %H:%M:%S").to_string());
                ProcessOut {
                    name: k.clone(),
                    state: v.state.clone(),
                    cmd: v.cmd.clone(),
                    pid: v.pid.unwrap_or(0),
                    start_time: start_time_str,
                    start_count: v.start_count,
                }
            })
            .collect()
    }
}

async fn supervise(cfg: ProcessConfig, registry: Arc<Registry>) {
    let (tx, mut rx) = mpsc::channel::<ControlMsg>(8);
    registry.register_process(&cfg.name, cfg.clone(), tx);

    loop {
        let start_time = tokio::time::Instant::now();

        let child = spawn_process(&cfg).unwrap();

        let pid = child.id();
        registry.set_running(&cfg.name, pid);
        tracing::info!("{} running with pid {}", cfg.name, pid);

        // 用 oneshot 接收 wait 结果
        let (exit_tx, mut exit_rx) = tokio::sync::oneshot::channel();

        // 把 wait 放到 blocking 线程，并且只在那里持有 child
        let mut wait_child = child;
        tokio::task::spawn_blocking(move || {
            let code = wait_child.wait().ok().and_then(|s| s.code()).unwrap_or(-1);
            let _ = exit_tx.send(code);
        });

        // 如果 cfg.max_run_time 有值，创建超时 future
        let max_run_fut = if let Some(max_time) = cfg.max_run {
            tokio::time::sleep(max_time)
        } else {
            // 永不超时
            tokio::time::sleep(Duration::from_secs(u64::MAX))
        };

        tokio::select! {
            // 子进程自然退出
            Result::Ok(code) = &mut exit_rx => {
                registry.set_state(&cfg.name, ProcState::Exited(code));
                tracing::info!("{} exited with {}", cfg.name, code);
            }

            // 收到控制命令
            Some(cmd) = rx.recv() => {
                match cmd {
                    ControlMsg::Kill | ControlMsg::Restart => {
                        tracing::info!("{} received kill", cfg.name);

                        kill_process(pid);

                        registry.set_state(&cfg.name, ProcState::Stopped);
                    }
                }
            }

                 // 达到最大运行时长
            _ = max_run_fut => {
                tracing::info!("{} reached max_run_time, killing process", cfg.name);
                kill_process(pid);
                registry.set_state(&cfg.name, ProcState::Stopped);
            }

        }

        let elapsed = start_time.elapsed();
        if elapsed < Duration::from_secs(1) {
            // 进程存活小于 1 秒 → sleep 1 秒
            sleep(Duration::from_secs(1)).await;
        }
    }
}

fn spawn_process(cfg: &ProcessConfig) -> anyhow::Result<std::process::Child> {
    let mut cmd = Command::new(&cfg.cmd);
    cmd.args(&cfg.args);
    for env in &cfg.envs {
        if let Some((key, value)) = env.split_once("=") {
            cmd.env(key, value);
        }
    }
    if !cfg.home.is_empty(){
        cmd.current_dir(&cfg.home);
    }

    if cfg.redirect_output {
        cmd.stdout(Stdio::piped()).stderr(Stdio::piped());
    }

    let mut child = match cmd.spawn() {
        Result::Ok(child) => {
            tracing::info!("spawn_process {} [ {:?} ] with pid {}",cfg.name.clone(), cmd, child.id());
            child
        }
        Result::Err(e) => {
            tracing::error!("spawn_process {} [ {:?} ] faild",cfg.name.clone(), cmd);
            return Err(anyhow::Error::new(e).context( format!("spawn_process {} failed",cfg.name.clone())));
        }
    };

    if cfg.redirect_output {
        if let Some(stdout) = child.stdout.take() {
            pipe_logger(stdout, cfg.clone(), "out");
        }
        if let Some(stderr) = child.stderr.take() {
            pipe_logger(stderr, cfg.clone(), "err");
        }
    }

    Ok(child)
}

fn current_hour() -> String {
    Local::now().format("%Y%m%d-%H").to_string()
}

fn pipe_logger( mut reader: impl std::io::Read + Send + 'static, cfg: ProcessConfig,kind: &'static str,) {
    std::thread::spawn(move || {
        let mut buf = [0u8; 4096];


        let mut file: Option<std::fs::File> = None;
        let mut active_hour = current_hour();

        loop {
           let n= match  reader.read(&mut buf){
                Ok(0)=>break, //  EOF
                Ok(n)=>n,
                Err(e)=>{
                     tracing::warn!("read pipe failed: {:?}", e);
                     break;
                }
            };

            let dir=Path::new(&cfg.output_dir);
            if !dir.exists(){
                match fs::create_dir_all(dir) {
                   Ok(())=>{},
                   Err(e)=>{
                      tracing::warn!("create log_dir_all {:?}",e.to_string());
                     break;
                   } 
                }
            }

            let hour = current_hour();
            let path = dir.join(format!("{kind}.{hour}.log"));
            let need_rotate = hour != active_hour;
            active_hour=hour;
        

            let missing= fs::metadata(&path).is_err();
    
            
            if missing|| need_rotate|| file.is_none(){
                match  OpenOptions::new().create(true).append(true).open(Path::new(&path)){
                    Ok(f) => {
                        file=Some(f)
                    },
                    Err(e) => {
                        tracing::warn!("open_log failed {:?}",e);
                    }
                };
            }

             if let Some(f) = file.as_mut() {
                if let Err(e) = f.write_all(&buf[..n]) {
                    tracing::warn!("write log failed: {:?}", e);
                    file = None
                }
            }
        
        }
    });
}

async fn list_processes(Extension(reg): Extension<Arc<Registry>>) -> Json<Vec<ProcessOut>> {
    Json(reg.list())
}

async fn restart_process(
    Extension(reg): Extension<Arc<Registry>>,
    extract::Path(name): extract::Path<String>,
) -> impl response::IntoResponse {
    // Logic to stop and restart the process
    tracing::info!("Restarting process: {}", name);
    // Placeholder: Just simulate the stop and start
    reg.set_state(&name, ProcState::Stopped);
    let reg = reg.as_ref();

    match reg.get_control(&name) {
        Some(tx) => {
            if let Err(e) = tx.send(ControlMsg::Restart).await {
                tracing::error!("failed to send restart to {}: {}", name, e);
                (
                    axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                    "failed to restart process",
                )
            } else {
                (axum::http::StatusCode::OK, "restart signal sent")
            }
        }
        None => (axum::http::StatusCode::NOT_FOUND, "process not found"),
    }
}

#[tokio::main]
async fn main() {
    let log_filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("trace,tower_http=trace"));

    tracing_subscriber::fmt().with_env_filter(log_filter).init();
    tracing::info!("starting ...");

    let mut config = Config {
        http: HttpConfig {
            addr: "127.0.0.1:8080".to_string(),
        },
        home:"/var/".to_string(),
        log_dir: "/var/log/procd".to_string(),
        processes: vec![ProcessConfig {
            name: "web-api".to_string(),
            cmd: "/usr/bin/python3".to_string(),
            args: vec![
                "-m".to_string(),
                "http.server".to_string(),
                "8090".to_string(),
            ],
            envs: vec![],
            output_dir: "".to_string(),
            home:"".to_string(),
            redirect_output: true,
            max_run: None,
        }],
    };

    for pc in config.processes.iter_mut() {
        if pc.output_dir.is_empty() {
            let mut path = std::path::PathBuf::from(&config.log_dir);
            path.push(&pc.name);
            pc.output_dir = path.to_string_lossy().to_string()
        }

        if pc.home.is_empty(){
            pc.home=config.home.clone();
        }
    }

    let registry = Arc::new(Registry::new());

    // Spawn processes
    for process_cfg in config.processes.clone() {
        let reg = registry.clone();
        tokio::spawn(supervise(process_cfg, reg));
    }

    // Set up web API
    let app: Router = Router::new()
        .route("/api/processes", get(list_processes))
        .route("/api/process/{name}/restart", post(restart_process))
        .layer(Extension(registry))
        .layer(TraceLayer::new_for_http());

    tracing::info!("Listening on {}", config.http.addr);

    let addr = &config.http.addr;
    let listener = tokio::net::TcpListener::bind(addr).await.unwrap();
    axum::serve(listener, app).await.unwrap();
}
