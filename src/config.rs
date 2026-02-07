use anyhow::Ok;
use serde::{Deserialize, Serialize};
use tokio::time::Duration;

#[derive(Debug, Deserialize, Clone)]
pub struct Config {
    pub http: HttpConfig,
    pub processes: Vec<ProcessConfig>,

    #[serde(default)]
    pub home: String,

    #[serde(default)]
    pub log_dir: String,

    #[serde(default)]
    pub auth: AuthConfig,
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

    #[serde(default)]
    pub max_run: Option<Duration>, // 最大运行时长，秒数
}

impl Config {
    pub fn check_and_init(&mut self) {
        if self.log_dir.is_empty() {
            self.log_dir = "logs".to_string()
        }
        for pc in self.processes.iter_mut() {
            if pc.output_dir.is_empty() {
                let mut path = std::path::PathBuf::from(&self.log_dir);
                path.push(&pc.name);
                pc.output_dir = path.to_string_lossy().to_string()
            }

            if pc.home.is_empty() {
                pc.home = self.home.clone();
            }
        }
    }

    pub fn from_file(path: &str) -> anyhow::Result<Config> {
        let settings = config::Config::builder()
            .add_source(config::File::with_name(path)) // 1. 加载文件
            .build()?;

        // 3. 转换成 struct
        let cfg: Config = settings.try_deserialize()?;
        Ok(cfg)
    }
}
