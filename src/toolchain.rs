use std::{env, process::Command};

use crate::display::terminal_safe;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolchainStatus {
    pub name: &'static str,
    pub version: Option<String>,
}

impl ToolchainStatus {
    pub fn available(&self) -> bool {
        self.version.is_some()
    }
}

pub fn detect_all() -> Vec<ToolchainStatus> {
    [
        "git", "node", "bun", "deno", "python3", "python", "npm", "pnpm", "yarn",
    ]
    .into_iter()
    .map(detect)
    .collect()
}

pub fn detect(name: &'static str) -> ToolchainStatus {
    let version = Command::new(name)
        .arg("--version")
        .output()
        .ok()
        .filter(|output| output.status.success())
        .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_string())
        .filter(|version| !version.is_empty());

    ToolchainStatus { name, version }
}

/// Cheap PATH lookup for binaries whose `--version` behavior varies,
/// such as `sh` and `git`.
pub fn exists_on_path(name: &str) -> bool {
    let Some(paths) = env::var_os("PATH") else {
        return false;
    };

    env::split_paths(&paths).any(|dir| {
        let candidate = dir.join(name);
        if candidate.is_file() {
            return true;
        }

        if cfg!(windows) {
            return dir.join(format!("{name}.exe")).is_file();
        }

        false
    })
}

pub fn render_doctor() -> String {
    let mut output = vec!["GitX doctor:".to_string()];

    for toolchain in detect_all() {
        output.push(format!(
            "  {:<5} {}",
            toolchain.name,
            toolchain
                .version
                .as_deref()
                .map(terminal_safe)
                .unwrap_or_else(|| "not found".to_string())
        ));
    }

    output.join("\n")
}
