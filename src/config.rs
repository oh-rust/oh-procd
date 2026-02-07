use serde::{Deserialize, Serialize};
use tokio::time::Duration;

#[derive(Debug, Deserialize, Clone)]
pub struct Config {
    pub http: HttpConfig,
    pub processes: Vec<ProcessConfig>,
    pub home: String,
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
    pub home: String,      // 进程根目录

    pub redirect_output: bool,     // 是否重定向 stdout 和 stderr 到日志
    pub output_dir: String,        // 单独的输出目录
    pub max_run: Option<Duration>, // 最大运行时长，秒数
}
