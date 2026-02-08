use std::process::{Command, Stdio};
use std::sync::Arc;
use tokio::{sync::mpsc, time::Duration};

use crate::{
    config::ProcessConfig,
    process::{
        logger::pipe_logger,
        registry::{ControlMsg, ProcState, Registry},
    },
};

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

fn spawn_process(cfg: &ProcessConfig) -> anyhow::Result<std::process::Child> {
    let mut cmd = Command::new(&cfg.cmd);
    cmd.args(&cfg.args);
    for env in &cfg.envs {
        if let Some((key, value)) = env.split_once("=") {
            cmd.env(key, value);
        }
    }
    if !cfg.home.is_empty() {
        cmd.current_dir(&cfg.home);
    }

    cmd.stdout(Stdio::piped()).stderr(Stdio::piped());

    let pid: u32;

    let mut child = match cmd.spawn() {
        Result::Ok(child) => {
            tracing::info!(
                "spawn_process {} [ {:?} ] with pid {}",
                cfg.name.clone(),
                cmd,
                child.id()
            );
            pid = child.id();
            child
        }
        Result::Err(e) => {
            tracing::error!("spawn_process {} [ {:?} ] faild", cfg.name.clone(), cmd);
            return Err(anyhow::Error::new(e).context(format!("spawn_process {} failed", cfg.name.clone())));
        }
    };

    if cfg.redirect_output {
        if let Some(stdout) = child.stdout.take() {
            pipe_logger(stdout, cfg.clone(), "out");
        }
        if let Some(stderr) = child.stderr.take() {
            pipe_logger(stderr, cfg.clone(), "err");
        }
    } else {
        if let Some(stdout) = child.stdout.take() {
            let name = cfg.name.clone();
            print_with_prefix(stdout, move |line| eprintln!("[{}/{}] {}", name.clone(), pid, line));
        }

        if let Some(stderr) = child.stderr.take() {
            let name = cfg.name.clone();
            print_with_prefix(stderr, move |line| println!("[{}/{}] {}", name.clone(), pid, line));
        }
    }

    Ok(child)
}

fn print_with_prefix(mut reader: impl std::io::Read + Send + 'static, output: impl Fn(&str) + Send + 'static) {
    std::thread::spawn(move || {
        let mut buf = [0u8; 4096];
        loop {
            let n = match reader.read(&mut buf) {
                Ok(0) => break, // EOF
                Ok(n) => n,
                Err(_) => break,
            };
            let s = String::from_utf8_lossy(&buf[..n]);
            for line in s.lines() {
                output(line);
            }
        }
    });
}

pub async fn supervise(cfg: ProcessConfig, registry: Arc<Registry>) {
    let (tx, mut rx) = mpsc::channel::<ControlMsg>(8);
    registry.register_process(&cfg.name, cfg.clone(), tx);

    loop {
        let start_time = tokio::time::Instant::now();

        let child = match spawn_process(&cfg) {
            Ok(c) => c,
            Err(_e) => {
                registry.set_state(&cfg.name, ProcState::Error);
                // 若启动失败，则等待 1 秒后重试
                tokio::time::sleep(Duration::from_secs(1)).await;
                continue;
            }
        };
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

        // 如果 cfg.max_run 有值，创建超时 future
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
                    ControlMsg::Kill  => {
                        tracing::info!("{} received kill", cfg.name);

                        kill_process(pid);

                        registry.set_state(&cfg.name, ProcState::Stopped);
                    }
                }
            }

                 // 达到最大运行时长
            _ = max_run_fut => {
                let elapsed = start_time.elapsed();
                tracing::info!("{} reached max_run_time (live={:?}), killing process", cfg.name,elapsed);
                kill_process(pid);
                registry.set_state(&cfg.name, ProcState::Stopped);
            }

        }

        let elapsed = start_time.elapsed();
        if elapsed < Duration::from_secs(1) {
            // 进程存活小于 1 秒 → sleep 1 秒
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
    }
}
