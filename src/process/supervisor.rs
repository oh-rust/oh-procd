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
use nix::sys::resource::{Resource, rlim_t, setrlimit};

#[cfg(unix)]
use nix::unistd::Pid;

#[cfg(windows)]
use windows_sys::Win32::System::Threading::{OpenProcess, TerminateProcess};

#[cfg(unix)]
fn kill_process(pid: u32) {
    if pid == 0 {
        return;
    }
    let _ = kill(Pid::from_raw(pid as i32), Signal::SIGKILL);
}

#[cfg(windows)]
fn kill_process(pid: u32) {
    if pid == 0 {
        return;
    }
    unsafe {
        let handle = OpenProcess(1, 0, pid);
        TerminateProcess(handle, 1);
    }
}

#[cfg(unix)]
use std::os::unix::process::CommandExt;

fn spawn_process(pcfg: &ProcessConfig) -> anyhow::Result<std::process::Child> {
    let mut cmd = Command::new(&pcfg.cmd);
    cmd.args(&pcfg.args);
    for env in &pcfg.envs {
        if let Some((key, value)) = env.split_once("=") {
            cmd.env(key, value);
        }
    }
    if !pcfg.home.is_empty() {
        cmd.current_dir(&pcfg.home);
    }

    #[cfg(unix)]
    {
        let mem_limit = pcfg.memory_limit.unwrap_or(0);
        unsafe {
            cmd.pre_exec(move || {
                libc::setsid();
                // 将自己加入一个新的进程组,子进程和所有子孙在同一进程组
                libc::setpgid(0, 0); // 0,0 表示自己作为 leader

                // 设置父死信号,父死子死
                #[cfg(target_os = "linux")]
                {
                    libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGTERM);
                }

                #[cfg(any(target_os = "linux", target_os = "macos"))]
                {
                    // 限制内存大小
                    if mem_limit > 0 {
                        let bytes: rlim_t = (mem_limit * 1024 * 1024) as u64;
                        setrlimit(Resource::RLIMIT_AS, bytes, bytes)
                            .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;
                    }
                }

                Ok(())
            });
        }
    }

    cmd.stdout(Stdio::piped()).stderr(Stdio::piped());

    let pid: u32;

    let mut child = match cmd.spawn() {
        Result::Ok(child) => {
            tracing::info!(
                "spawn_process {} [ {:?} ] with pid {}",
                pcfg.name.clone(),
                cmd,
                child.id()
            );
            pid = child.id();
            child
        }
        Result::Err(e) => {
            let msg = format!("spawn_process {} [ {:?} ] faild: {:?}", pcfg.name.clone(), cmd, e);
            tracing::error!(msg);
            return Err(anyhow::Error::new(e).context(msg));
        }
    };

    if pcfg.redirect_output {
        if let Some(stdout) = child.stdout.take() {
            pipe_logger(stdout, pcfg.clone(), "out");
        }
        if let Some(stderr) = child.stderr.take() {
            pipe_logger(stderr, pcfg.clone(), "err");
        }
    } else {
        if let Some(stdout) = child.stdout.take() {
            let name = pcfg.name.clone();
            print_with_prefix(stdout, move |line| {
                tracing::info!(from = "stdout", pid = pid, name = name.clone(), "{}", line)
            });
        }

        if let Some(stderr) = child.stderr.take() {
            let name = pcfg.name.clone();
            print_with_prefix(stderr, move |line| {
                tracing::info!(from = "stderr", pid = pid, name = name.clone(), "{}", line);
            });
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

    // 如果 cfg.next 有值
    let wait_next = || async {
        if let Some(next) = cfg.next {
            tokio::time::sleep(next).await;
        }
    };

    loop {
        let start_time = tokio::time::Instant::now();

        let child = match spawn_process(&cfg) {
            Ok(c) => c,
            Err(e) => {
                registry.set_state(&cfg.name, ProcState::Error(e.to_string()));
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
                wait_next().await;
            }

            // 收到控制命令
            Some(cmd) = rx.recv() => {
                match cmd {
                    ControlMsg::Restart  => {
                        tracing::info!("{} received restart", cfg.name);
                        kill_process(pid);
                        registry.set_state(&cfg.name, ProcState::Stopped);
                        // 主动重启的，不需要 wait_next
                    }
                    ControlMsg::Kill =>{
                        tracing::info!("{} received kill", cfg.name);
                        kill_process(pid);
                        registry.set_state(&cfg.name, ProcState::Killed);
                        return   // 主动杀死的，退出循环
                    }
                }
            }

            // 达到最大运行时长
            _ = max_run_fut => {
                let elapsed = start_time.elapsed();
                tracing::info!("{} reached max_run_time (live={:?}), killing process", cfg.name,elapsed);
                kill_process(pid);
                registry.set_state(&cfg.name, ProcState::Stopped);
                wait_next().await;
            }

        }

        let elapsed = start_time.elapsed();
        if elapsed < Duration::from_secs(1) {
            // 进程存活小于 1 秒 → sleep 1 秒, 避免平凡启动进程，导致 cpu 100%
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
    }
}
