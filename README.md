# Oh-Procd

**oh-procd** is a lightweight and flexible process manager written in Rust. It helps developers run, monitor, and manage multiple processes efficiently with features like automatic restart, logging, and environment configuration.

---

## Features

- 🚀 **Process Management**: Start and monitor multiple processes easily.  
- 🔄 **Auto Restart**: Automatically restart processes when they exit unexpectedly.  
- 📅 **Logging & Rotation**: Capture process output and support log rotation.  
- 🛠 **Configurable**: YAML/JSON-based configuration for commands, environment variables, and working directories.  
- 🔧 **Lightweight & Efficient**: Built with Rust for high performance and low overhead.

---

## Installation

Build **oh-procd** from source using Rust's `cargo`:

```bash
git clone https://github.com/oh-rust/oh-procd.git
cd oh-procd
cargo build --release
```

The compiled binary will be located at:
```
target/release/oh-procd
```


## Usage

Prepare a configuration file (e.g., [procd.yaml](procd.yaml) ):
```yaml
# Required configuration: HTTP server for the management page
http:
  addr: "127.0.0.1:8080" 

# Optional configuration: authentication account
# auth:
#   username: admin
#   password: 123

processes:
  - name: web-api
    cmd: "python3"
    args: ["-m", "http.server","8090"]
    home: /tmp
    # max_run: "10s"  # Maximum continuous runtime
    # next: "30s" # Wait time before next run after exit

  - name: sleep
    cmd: "sleep"
    args: ["10"]
    home: /tmp
    # max_run: "10s"  # Maximum continuous runtime
    next: "30s" # Wait time before next run after exit

```

Run oh-procd with your configuration:
```bash
./oh-procd -c procd.yaml
```