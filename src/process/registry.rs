use chrono::{DateTime, Local};
use serde::Serialize;
use std::collections::hash_map::Entry;
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
    pub pid: Option<u32>,
    pub control_tx: mpsc::Sender<ControlMsg>,
    pub start_time: Option<DateTime<Local>>, // 进程启动时间
    pub start_count: u64,                    // 程序启动次数
    pub exit_time: Option<DateTime<Local>>,  // 进程上次退出时间
}

pub struct Registry {
    start: DateTime<Local>,
    inner: Arc<Mutex<HashMap<String, ProcessEntry>>>,
}

#[derive(Serialize, Clone, Debug)]
pub struct ProcessOut {
    pub name: String,
    pub cmd: ProcessConfig,
    pub state: ProcState,
    pub pid: u32,
    pub start_time: Option<String>,
    pub start_count: u64,
    pub exit_time: Option<String>,
    pub memory_limit: u32,
    pub memory_used: String,
}

impl Registry {
    pub fn new() -> Self {
        Registry {
            start: Local::now(),
            inner: Arc::new(Mutex::new(HashMap::new())),
        }
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
                e.insert(ProcessEntry {
                    index: index + 1,
                    state: ProcState::Ready,
                    cmd: cmd,
                    pid: None,
                    control_tx: tx,
                    start_time: None,
                    start_count: 0,
                    exit_time: None,
                });
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

            tracing::info!("set_state {} -> ({:?}, {:?})", name, state, entry.pid.unwrap_or(0));
        } else {
            panic!("set_state {} not found", name);
        }
    }

    pub fn set_running(&self, name: &str, pid: u32) {
        let mut registry = self.inner.lock().unwrap();
        if let Some(entry) = registry.get_mut(name) {
            entry.state = ProcState::Running;
            entry.pid = Some(pid);
            tracing::info!("set_state {} -> ({:?}, {:?})", name, ProcState::Running, pid);
            entry.start_time = Some(Local::now());
            entry.start_count += 1;
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
        tracing::info!("list.size={}", entries.len());

        entries
            .into_iter()
            .map(|(k, v)| {
                let start_time_str = v.start_time.map(|t| t.format("%Y-%m-%d %H:%M:%S").to_string());
                let exit_time_str = v.exit_time.map(|t| t.format("%Y-%m-%d %H:%M:%S").to_string());
                ProcessOut {
                    name: k.clone(),
                    state: v.state.clone(),
                    cmd: v.cmd.clone(),
                    pid: v.pid.unwrap_or(0),
                    start_time: start_time_str,
                    start_count: v.start_count,
                    exit_time: exit_time_str,
                    memory_limit: v.cmd.memory_limit.unwrap_or(0),
                    memory_used: "".to_string(),
                }
            })
            .collect()
    }

    pub fn start_time(&self) -> String {
        self.start.format("%Y-%m-%d %H:%M:%S").to_string()
    }
}
