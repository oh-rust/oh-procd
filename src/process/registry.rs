use chrono::{DateTime, Local};
use serde::Serialize;
use std::collections::hash_map::Entry;
use std::fmt::Debug;
use std::time::Duration;
use std::time::SystemTime;
use std::{collections::HashMap, sync::Arc, sync::Mutex};
use tokio::sync::mpsc;

use crate::config::ProcessConfig;

#[derive(Serialize, Clone, Debug, PartialEq)]

pub enum ProcState {
    Ready,    // 就绪
    Running,  // 正常运行
    Stopping, // 即将停止，收到 Kill 和 Restart 命令了

    Error(String), // 启动失败
    Stopped,       // 停止
    Killed,        // 被手动(使用 API)杀死了
    Exited(i32),   // 程序自己退出
}

pub enum ControlMsg {
    Kill,    // 杀死进程，后续不会继续运行
    Restart, // 重启进程
}

#[derive(Clone)]
pub struct ProcessEntry {
    pub index: i32, // 排序
    pub state: ProcState,
    pub cmd: ProcessConfig,
    pub cmd_abs_path: Option<String>, //命令的绝对地址
    pub pid: Option<u32>,
    pub control_tx: mpsc::Sender<ControlMsg>,
    pub start_time: Option<DateTime<Local>>, // 进程启动时间
    pub start_count: u64,                    // 程序启动次数
    pub exit_time: Option<DateTime<Local>>,  // 进程上次退出时间
    pub last_modified: Option<SystemTime>,   // cmd 文件启动时的修改时间
}

pub struct Registry {
    start: DateTime<Local>,
    inner: Arc<Mutex<HashMap<String, ProcessEntry>>>,
}

#[derive(Serialize, Clone, Debug)]
pub struct ProcessOut {
    pub name: String,
    pub cmd: ProcessConfig,
    pub cmd_abs: String,
    pub state: ProcState,
    pub pid: u32,
    pub start_time: Option<String>,
    pub start_count: u64,
    pub exit_time: Option<String>,
    pub memory_limit: u32,
    pub memory_used: String,
    pub web_address: String,
    pub sandbox: bool,         // 使用启用沙盒
    pub mtime: Option<String>, // cmd 文件的最后修改时间
    pub child_pids: Vec<u32>,  // 子进程的 pid 列表
}

impl ProcessEntry {
    fn get_cmd_mtime(&self) -> Option<std::time::SystemTime> {
        if self.cmd_abs_path.is_none() {
            return None;
        }

        let path = self.cmd_abs_path.clone().unwrap();
        std::fs::metadata(&path)
            .and_then(|m| m.modified())
            .map_err(|e| {
                tracing::warn!("read metadata({}) failed: {:?}", &path, e);
            })
            .ok()
    }
}

const TIME_FMT: &str = "%Y-%m-%d %H:%M:%S";

impl Registry {
    pub fn new() -> Self {
        Registry {
            start: Local::now(),
            inner: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    // 监听二进制文件变化，并自动 restart
    pub fn watch(self: Arc<Self>, dur: Duration) {
        if dur.as_secs() < 1 {
            tracing::info!("restart_delay is disabled, Duration={:?}", dur);
            return;
        }
        tracing::info!("restart_delay is enable, watch Duration={:?}", dur);
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(dur).await;
                let names: Vec<String> = self.inner.lock().unwrap().keys().cloned().collect();
                for name in &names {
                    self.watch_one(&name);
                }
            }
        });
    }

    fn watch_one(&self, name: &str) {
        let entry = self.find(name);
        if entry.is_none() {
            tracing::warn!("watch_one  find({}) is null", name);
            return;
        }
        let pe = entry.unwrap();

        if !pe.cmd.enable || pe.state != ProcState::Running {
            return;
        }
        if pe.cmd_abs_path.is_none() {
            tracing::warn!(name, "watch_one cmd_abs_path is null");
            return;
        }

        let current_mtime = pe.get_cmd_mtime();
        if current_mtime.is_none() {
            tracing::warn!("watch_one({}) get_current_mtime is null", name);
        }
        if current_mtime == pe.last_modified {
            return;
        }
        tracing::info!(
            "watch_one({}) file changed: {} (previous: {:?}, now: {:?})",
            name,
            pe.cmd.cmd,
            pe.last_modified,
            current_mtime
        );
        let _ = pe.control_tx.clone().try_send(ControlMsg::Restart);
    }

    pub fn find(&self, name: &str) -> Option<ProcessEntry> {
        self.inner.lock().unwrap().get(name).cloned()
    }

    pub fn register_process(&self, name: &str, cmd: ProcessConfig, tx: mpsc::Sender<ControlMsg>) {
        let mut registry = self.inner.lock().unwrap();
        let index: i32 = registry.len() as i32;

        match registry.entry(name.to_string()) {
            Entry::Occupied(mut e) => {
                e.get_mut().control_tx = tx;
                tracing::info!("register_process_update {}", name);
            }
            Entry::Vacant(e) => {
                let abs_path: Option<String> = cmd.cmd_abs_path().ok().map(|p| p.to_string_lossy().to_string());

                let mut pe = ProcessEntry {
                    index: index + 1,
                    state: ProcState::Ready,
                    cmd: cmd,
                    cmd_abs_path: abs_path,
                    pid: None,
                    control_tx: tx,
                    start_time: None,
                    start_count: 0,
                    exit_time: None,
                    last_modified: None,
                };
                pe.last_modified = pe.get_cmd_mtime();

                e.insert(pe);
                tracing::info!("register_process_insert {}", name);
            }
        }
    }

    pub fn get_control(&self, name: &str) -> Option<tokio::sync::mpsc::Sender<ControlMsg>> {
        self.inner.lock().unwrap().get(name).map(|e| e.control_tx.clone())
    }

    pub fn set_state(&self, name: &str, state: ProcState) {
        let mut registry = self.inner.lock().unwrap();
        if let Some(entry) = registry.get_mut(name) {
            entry.state = state.clone();

            if matches!(
                state.clone(),
                ProcState::Stopped | ProcState::Killed | ProcState::Exited(_) | ProcState::Error(_)
            ) {
                entry.exit_time = Some(Local::now());
            }

            if matches!(state.clone(), ProcState::Error(_)) {
                entry.start_count += 1;
            }

            tracing::info!("set_state -> ({}, {:?}, {:?})", name, state, entry.pid.unwrap_or(0));
        } else {
            panic!("set_state {} not found", name);
        }
    }

    pub fn set_running(&self, name: &str, pid: u32) {
        let mut registry = self.inner.lock().unwrap();
        if let Some(entry) = registry.get_mut(name) {
            entry.state = ProcState::Running;
            entry.pid = Some(pid);
            tracing::info!("set_state -> ({}, {:?}, {:?})", name, ProcState::Running, pid);
            entry.start_time = Some(Local::now());
            entry.start_count += 1;

            entry.last_modified = entry.get_cmd_mtime(); // 运行后，立即更新文件时间
        } else {
            panic!("set_running {} not found", name)
        }
    }

    pub fn list(&self) -> Vec<ProcessOut> {
        let entries: Vec<(String, ProcessEntry)> = {
            let registry = self.inner.lock().unwrap();
            let mut ret: Vec<_> = registry.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
            ret.sort_by_key(|(_, v)| v.index);
            ret
        };

        entries
            .into_iter()
            .map(|(k, v)| {
                let start_time_str = v.start_time.map(|t| t.format(TIME_FMT).to_string());
                let exit_time_str = v.exit_time.map(|t| t.format(TIME_FMT).to_string());
                let mtime_str: Option<String> = v.last_modified.map(|t| {
                    let dt: DateTime<Local> = t.into();
                    dt.format(TIME_FMT).to_string()
                });
                ProcessOut {
                    name: k.clone(),
                    state: v.state.clone(),
                    cmd: v.cmd.clone(),
                    cmd_abs: v.cmd_abs_path.unwrap_or("".to_string()),
                    pid: v.pid.unwrap_or(0),
                    start_time: start_time_str,
                    start_count: v.start_count,
                    exit_time: exit_time_str,
                    memory_limit: v.cmd.memory_limit.unwrap_or(0),
                    memory_used: "".to_string(),
                    web_address: v.cmd.web_address.clone(),
                    sandbox: !v.cmd.sandbox.is_empty(),
                    mtime: mtime_str,
                    child_pids: vec![],
                }
            })
            .collect()
    }

    pub fn start_time(&self) -> String {
        self.start.format("%Y-%m-%d %H:%M:%S").to_string()
    }
}
