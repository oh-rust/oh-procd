use chrono::{DateTime, Local};
use serde::Serialize;
use std::{collections::HashMap, sync::Mutex};
use tokio::sync::mpsc;

use crate::config::ProcessConfig;

#[derive(Serialize, Clone, Debug)]
pub enum ProcState {
    Ready,
    Running,
    Stopped,
    Exited(i32),
}

pub enum ControlMsg {
    Kill,
}

pub struct ProcessEntry {
    pub state: ProcState,
    pub cmd: ProcessConfig,
    pub pid: Option<u32>,
    pub control_tx: mpsc::Sender<ControlMsg>,
    pub start_time: Option<DateTime<Local>>,
    pub start_count: u64,
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
