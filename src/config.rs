use std::{path::Path, sync::Arc};

use anyhow::Ok;
use serde::{Deserialize, Serialize};
use tokio::time::Duration;

#[derive(Debug, Deserialize, Clone)]
pub struct Config {
    pub http: HttpConfig, // web server 配置

    pub processes: Vec<ProcessConfig>, // 子进程配置列表

    #[serde(default)]
    pub home: String, // 默认的工作目录

    #[serde(default)]
    pub log_dir: String, // 日志目录

    #[serde(default)]
    pub auth: AuthConfig, // web 页面认证信息

    #[serde(default)]
    pub envs: Vec<String>, // 传递给子进程的环境变量配置

    #[serde(default, with = "humantime_serde::option")]
    pub restart_on_change: Option<Duration>, // 文件变化后，延迟重启的时间间隔
}

#[derive(Debug, Deserialize, Clone)]
pub struct HttpConfig {
    pub addr: String,
}

#[derive(Debug, Deserialize, Clone, Default)]
#[serde(default)]
pub struct AuthConfig {
    pub username: String,
    pub password: String,
}

impl AuthConfig {
    pub fn check(&self, name: &str, psw: &str) -> bool {
        return self.username == name && self.password == psw;
    }
}

#[derive(Serialize, Debug, Deserialize, Clone)]
pub struct ProcessConfig {
    pub name: String,
    pub cmd: String,
    pub args: Vec<String>,

    #[serde(default)]
    pub envs: Vec<String>, // 额外的环境变量值

    #[serde(default)]
    pub home: String, // 进程根目录

    #[serde(default)]
    pub redirect_output: bool, // 是否重定向 stdout 和 stderr 到日志

    #[serde(default)]
    pub output_dir: String, // 单独的输出目录

    #[serde(default, with = "humantime_serde::option")]
    pub max_run: Option<Duration>, // 最大运行时长，秒数，配置文件配置值 "10s"、"1h30m"

    #[serde(default, with = "humantime_serde::option")]
    pub restart_on_change: Option<Duration>, // 文件变化后，延迟重启的时间间隔

    #[serde(default, with = "humantime_serde::option")]
    pub next: Option<Duration>, // 下一次运行距离上次退出的时间间隔
}

impl Config {
    fn check_and_init(&mut self) {
        if self.log_dir.is_empty() {
            self.log_dir = "logs".to_string()
        }

        for pc in self.processes.iter_mut() {
            // 合并全局的环境变量
            let mut merged = self.envs.clone();
            merged.extend(pc.envs.clone());
            pc.envs = merged;

            if pc.output_dir.is_empty() {
                let mut path = std::path::PathBuf::from(&self.log_dir);
                path.push(&pc.name);
                pc.output_dir = path.to_string_lossy().to_string()
            }

            if pc.home.is_empty() {
                pc.home = self.home.clone();
            }

            if !self.restart_on_change.is_none() && pc.restart_on_change.is_none() {
                pc.restart_on_change = self.restart_on_change.clone();
            }
        }
    }

    pub fn from_file(path: &str) -> anyhow::Result<Config> {
        let settings = config::Config::builder()
            .add_source(config::File::with_name(path)) // 1. 加载文件
            .build()?;

        // 3. 转换成 struct
        let mut cfg: Config = settings.try_deserialize()?;

        if cfg.home.is_empty() {
            let fp = Path::new(path);
            cfg.home = fp
                .parent()
                .map(|p| p.to_string_lossy().to_string())
                .ok_or_else(|| anyhow::anyhow!("invalid path: no parent directory"))?;
        }
        cfg.check_and_init();
        Ok(cfg)
    }
}

use crate::process;
use crate::process::registry::Registry;

impl ProcessConfig {
    pub fn start_spawn(&self, reg: Arc<Registry>) {
        let cfg = self.clone();
        tokio::spawn(process::supervisor::supervise(cfg, reg));
    }
}
