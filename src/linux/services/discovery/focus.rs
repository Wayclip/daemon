use std::{
    env,
    io::{Read, Write},
    os::unix::net::UnixStream,
    time::Duration,
};

#[derive(serde::Deserialize)]
struct ActiveWindow {
    pid: i64,
}

pub struct WindowFocusScanner;

const HYPR_SIGNATURE: &str = "HYPRLAND_INSTANCE_SIGNATURE";
const SWAY_SIGNATURE: &str = "SWAYSOCK";

impl WindowFocusScanner {
    pub fn focused_pid() -> Option<u32> {
        if let Some(sig) = env::var_os(HYPR_SIGNATURE) {
            return Self::hyprland(&sig.as_os_str().to_string_lossy());
        }
        if let Some(sig) = env::var_os(SWAY_SIGNATURE) {
            return Self::sway(&sig.as_os_str().to_string_lossy());
        }
        None
    }

    fn hyprland(sig: &str) -> Option<u32> {
        let xdg_runtime = dirs::runtime_dir().unwrap_or_default();
        let socket = format!(
            "{}/hypr/{}/.socket.sock",
            xdg_runtime.to_string_lossy(),
            sig
        );

        let mut stream = UnixStream::connect(socket).ok()?;
        stream
            .set_read_timeout(Some(Duration::from_millis(100)))
            .ok()?;
        stream.write_all(b"j/activewindow").ok()?;

        let mut res = Vec::new();
        stream.read_to_end(&mut res).ok()?;

        let parsed: ActiveWindow = serde_json::from_slice(&res).ok()?;
        if parsed.pid > 0 {
            Some(parsed.pid as u32)
        } else {
            None
        }
    }

    fn sway(sig: &str) -> Option<u32> {
        let mut stream = UnixStream::connect(sig).ok()?;
        stream
            .set_read_timeout(Some(Duration::from_millis(100)))
            .ok()?;

        let mut payload = Vec::with_capacity(14);
        payload.extend_from_slice(b"i3-ipc");
        payload.extend_from_slice(&0u32.to_le_bytes());
        payload.extend_from_slice(&4u32.to_le_bytes());

        stream.write_all(&payload).ok()?;
        let mut header = [0u8; 14];
        stream.read_exact(&mut header).ok()?;
        if &header[0..6] != b"i3-ipc" {
            return None;
        }

        let len = u32::from_le_bytes(header[6..10].try_into().ok()?) as usize;
        let mut body = vec![0u8; len];
        stream.read_exact(&mut body).ok()?;

        let tree: serde_json::Value = serde_json::from_slice(&body).ok()?;
        Self::search_node(&tree)
    }

    fn search_node(node: &serde_json::Value) -> Option<u32> {
        if node["focused"] == true {
            return node["pid"].as_u64().map(|p| p as u32);
        }

        ["nodes", "floating_nodes"]
            .into_iter()
            .filter_map(|k| node[k].as_array())
            .flatten()
            .find_map(Self::search_node)
    }
}
