use std::{env, fs, sync::Arc};

use anyhow::Ok;
use serde::{Deserialize, Serialize};
use tokio::time::Duration;

use crate::process;
use crate::process::registry::Registry;

#[derive(Debug, Deserialize, Clone)]
pub struct Config {
    pub http: HttpConfig, // web server 配置

    pub process: Vec<ProcessConfig>, // 子进程配置列表

    #[serde(default)]
    pub sandbox: Vec<SandboxConfig>, // 沙盒环境配置列表

    #[serde(default)]
    pub home: String, // 默认的工作目录

    #[serde(default)]
    pub log_dir: String, // 日志目录

    #[serde(default)]
    pub auth: AuthConfig, // web 页面认证信息

    #[serde(default)]
    pub envs: Vec<String>, // 传递给子进程的环境变量配置

    #[serde(default, with = "humantime_serde::option")]
    pub restart_delay: Option<Duration>, // 文件变化后，延迟重启的时间间隔

    #[serde(default = "default_true")]
    pub enable_sandbox: bool, // 是否进入沙盒以安全运行,若为false，则所有子进程都为 false
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

#[derive(Debug, Deserialize, Clone)]
pub struct SandboxConfig {
    pub name: String,
    pub cmd: Vec<String>, // 程序命令和参数

    #[serde(default = "default_true")]
    pub enable: bool, // 该配置是否启用，默认为 true
}

impl SandboxConfig {
    fn get_cmd(&self) -> Vec<String> {
        if self.cmd.is_empty() {
            return vec![];
        }

        // 将沙盒程序的命令转换为绝对路径
        let first = which::which(self.cmd[0].clone())
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_else(|_e| self.cmd[0].clone());

        let mut cmd = self.cmd.clone();
        cmd[0] = first;
        return cmd;
    }
}

#[derive(Serialize, Debug, Deserialize, Clone)]
pub struct ProcessConfig {
    pub name: String,

    pub cmd: String, // 程序命令，必填

    #[serde(default)]
    pub args: Vec<String>, // 命令参数，可选

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
    pub next: Option<Duration>, // 下一次运行距离上次退出的时间间隔

    #[serde(default)]
    pub memory_limit: Option<u32>, // 内存限制,单位 MB

    #[serde(default)]
    pub web_address: String, // 通过管理页面访问的地址，支持变量 ${HOST}

    #[serde(default = "default_true")]
    pub enable: bool, // 该配置是否启用，默认为 true

    #[serde(default)]
    pub use_sandbox: String, // 使用沙盒的名称

    #[serde(default)]
    pub sandbox: Vec<String>, // 沙盒的命令
}

fn default_true() -> bool {
    true
}

impl Config {
    fn check_and_init(&mut self) {
        if self.log_dir.is_empty() {
            self.log_dir = "logs".to_string()
        }

        let sbox = self.sandbox.clone();
        for pc in self.process.iter_mut() {
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

            if !self.enable_sandbox || pc.use_sandbox == "no" {
                pc.sandbox.clear();
            } else if pc.sandbox.is_empty() {
                let name = &pc.use_sandbox;
                if let Some(c) = sbox.iter().find(|c| c.enable && (c.name.eq(name) || name.is_empty())) {
                    pc.sandbox = c.get_cmd();
                } else {
                    tracing::warn!("use_sandbox={} not found, skipped", &pc.use_sandbox)
                }
            }
        }
    }

    pub fn from_file(path: &str) -> anyhow::Result<Config> {
        let settings = config::Config::builder()
            .add_source(config::File::with_name(path)) // 1. 加载文件
            .build()?;

        // 3. 转换成 struct
        let mut cfg: Config = settings.try_deserialize()?;

        cfg.check_and_init();
        Ok(cfg)
    }

    pub fn set_current_dir(&self, cfg_path: &str) -> anyhow::Result<()> {
        // 切换工作目录到配置文件所在目录
        {
            use anyhow::Context;
            let abs_path = std::fs::canonicalize(cfg_path)
                .with_context(|| anyhow::anyhow!("failed to canonicalize path {}", cfg_path))?;
            let parent = abs_path
                .parent()
                .ok_or_else(|| anyhow::anyhow!("has no parent directory"))?;
            tracing::debug!("config dir:{}", parent.display());
            env::set_current_dir(parent)?;
        }

        // 切换工作目录到配置指定的目录
        {
            let home = self.home.clone();
            if !home.is_empty() {
                env::set_current_dir(home.clone())?;
            }
        }

        // 打印当前目录
        let dir = env::current_dir()?;
        tracing::info!("current_dir: {}", dir.display());
        Ok(())
    }
}

impl ProcessConfig {
    pub fn start_spawn(&self, reg: Arc<Registry>) {
        let cfg = self.clone();
        tokio::spawn(process::supervisor::supervise(cfg, reg));
    }

    pub fn mtime(&self) -> Option<std::time::SystemTime> {
        let path = which::which(&self.cmd).ok();
        if path.is_none() {
            return None;
        }
        fs::metadata(path.unwrap())
            .and_then(|m| m.modified())
            .map_err(|e| {
                tracing::warn!("read metadata({}) failed: {:?}", self.cmd, e);
            })
            .ok()
    }

    // pub fn cmd_path(&self) -> String {
    //     which::which(&self.cmd)
    //         .map(|p| p.to_string_lossy().into_owned())
    //         .unwrap_or_else(|_e| self.cmd.clone())
    // }

    pub fn get_cmd(&self) -> std::process::Command {
        let mut args = self.sandbox.clone();
        args.push(self.cmd.clone());
        for a in &self.args.clone() {
            args.push(a.clone());
        }

        let mut app_home: String = env::current_dir().unwrap().to_string_lossy().to_string();
        if !self.home.is_empty() {
            app_home = self.home.clone();
        }

        let mut has_replace = false;
        for a in args.iter_mut() {
            if a.contains("{Process-Home}") {
                *a = a.replace("{Process-Home}", &app_home);
                has_replace = true;
            }
        }

        tracing::debug!("cmd: {}", args.clone().join(" "));

        let mut cmd = std::process::Command::new(&args[0]);
        cmd.args(&args[1..]);

        cmd.env("NO_COLOR", "1"); // 子进程不输出颜色
        for env in &self.envs {
            if let Some((key, value)) = env.split_once("=") {
                cmd.env(key, value);
            }
        }
        if !has_replace && !self.home.is_empty() {
            cmd.current_dir(&self.home);
        }
        return cmd;
    }
}
