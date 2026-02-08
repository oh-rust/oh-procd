use std::{
    collections::VecDeque,
    sync::{Arc, Mutex},
};
use tracing_subscriber::Layer;
use tracing_subscriber::{EnvFilter, layer::SubscriberExt};

#[derive(Clone)]
pub struct LogBuffer {
    buffer: Arc<Mutex<VecDeque<String>>>,
    capacity: usize,
}

impl LogBuffer {
    fn new(capacity: usize) -> Self {
        Self {
            buffer: Arc::new(Mutex::new(VecDeque::with_capacity(capacity))),
            capacity,
        }
    }

    fn push(&self, msg: String) {
        let mut buf = self.buffer.lock().unwrap();
        if buf.len() == self.capacity {
            buf.pop_front();
        }
        buf.push_back(msg);
    }

    pub fn get_logs(&self) -> Vec<String> {
        let buf = self.buffer.lock().unwrap();
        buf.iter().cloned().collect()
    }
}

struct StringVisitor<'a> {
    output: &'a mut String,
}

use tracing::field::{Field, Visit};

impl<'a> Visit for StringVisitor<'a> {
    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        if !self.output.is_empty() {
            self.output.push_str(" ");
        }
        // 如果字段名是 "message"，通常我们不打印 "message=" 前缀
        if field.name() == "message" {
            self.output.push_str(&format!("{:?}", value));
        } else {
            self.output.push_str(&format!("{}={:?}", field.name(), value));
        }
    }
}

use std::sync::atomic::{AtomicU64, Ordering};

struct BufferLayer {
    pub buffer: LogBuffer,

    pub counter: AtomicU64,
}

impl<S> Layer<S> for BufferLayer
where
    S: tracing::Subscriber,
{
    fn on_event(&self, event: &tracing::Event<'_>, _ctx: tracing_subscriber::layer::Context<'_, S>) {
        // let msg = format!("{:?}", event);
        // self.buffer.push(msg);

        //  序列化为人可读的字符串
        let now = chrono::Local::now().format("%Y-%m-%d %H:%M:%S%.3f");

        let count = self.counter.fetch_add(1, Ordering::Relaxed);

        let mut fields_string = String::new();
        let mut visitor = StringVisitor {
            output: &mut fields_string,
        };

        // 2. 调用 record 处理 event 里的所有字段
        event.record(&mut visitor);

        // 3. 拼接元数据（级别、目标等）
        let metadata = event.metadata();
        let human_readable_msg = format!(
            "[{}] {} [{}] {}: {}",
            count,
            now,
            metadata.level().to_string(),
            metadata.target(),
            fields_string
        );

        self.buffer.push(human_readable_msg);
    }
}

pub fn new_logbuf() -> LogBuffer {
    let log_buffer = LogBuffer::new(100);
    let layer = BufferLayer {
        buffer: log_buffer.clone(),
        counter: AtomicU64::new(0),
    };
    let log_filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("trace,tower_http=trace"));
    let subscriber = tracing_subscriber::fmt()
        .with_env_filter(log_filter)
        .finish()
        .with(layer);
    tracing::subscriber::set_global_default(subscriber).unwrap();

    log_buffer
}
