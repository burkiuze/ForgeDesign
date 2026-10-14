//! Updates from GitHub.
//!
//! Two channels:
//! * **Release** (default): checks the latest GitHub Release, downloads the
//!   `*-Setup.exe` asset and runs the setup wizard. No Rust needed.
//! * **Source**: downloads the newest source code from the `main` branch and
//!   builds it with Cargo. Rust is installed automatically through rustup
//!   when it is not already on the computer.
//!
//! All network and build work runs on a background thread; the UI polls
//! messages from a channel.

use std::io::{BufRead, Read};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc::{channel, Receiver, Sender};

pub const REPO: &str = "burkiuze/ForgeDesign";
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

#[derive(Clone, Debug)]
pub struct Release {
    pub version: String,
    pub notes: String,
    pub page: String,
    pub installer: Option<String>,
}

enum Msg {
    Log(String),
    Checked(Result<Option<Release>, String>),
    Finished(Result<String, String>),
}

#[derive(Clone, Debug, PartialEq)]
pub enum State {
    Idle,
    Checking,
    UpToDate,
    Available,
    Working,
    Done(String),
    Failed(String),
}

pub struct Updater {
    rx: Option<Receiver<Msg>>,
    pub state: State,
    pub release: Option<Release>,
    pub log: Vec<String>,
    /// Show the Updates window.
    pub open: bool,
    /// A silent startup check only opens the window when an update exists.
    silent: bool,
}

impl Default for Updater {
    fn default() -> Self {
        Self { rx: None, state: State::Idle, release: None, log: Vec::new(), open: false, silent: false }
    }
}

/// "v1.2.3" / "1.2.3-beta" → (1, 2, 3).
pub fn parse_version(s: &str) -> (u32, u32, u32) {
    let s = s.trim().trim_start_matches(['v', 'V']);
    let core = s.split(['-', '+']).next().unwrap_or("");
    let mut it = core.split('.').map(|p| p.parse::<u32>().unwrap_or(0));
    (it.next().unwrap_or(0), it.next().unwrap_or(0), it.next().unwrap_or(0))
}

pub fn is_newer(remote: &str, local: &str) -> bool {
    parse_version(remote) > parse_version(local)
}

fn agent() -> Result<ureq::Agent, String> {
    #[cfg(windows)]
    {
        let tls = native_tls::TlsConnector::new().map_err(|e| e.to_string())?;
        Ok(ureq::AgentBuilder::new().tls_connector(std::sync::Arc::new(tls)).user_agent("ForgeDesign-Updater").build())
    }
    #[cfg(not(windows))]
    {
        Ok(ureq::AgentBuilder::new().user_agent("ForgeDesign-Updater").build())
    }
}

fn fetch_latest() -> Result<Option<Release>, String> {
    let url = format!("https://api.github.com/repos/{REPO}/releases/latest");
    let resp = match agent()?.get(&url).set("Accept", "application/vnd.github+json").call() {
        Ok(r) => r,
        Err(ureq::Error::Status(404, _)) => return Ok(None), // no releases published yet
        Err(e) => return Err(format!("GitHub could not be reached: {e}")),
    };
    let v: serde_json::Value = resp.into_json().map_err(|e| e.to_string())?;
    let tag = v["tag_name"].as_str().unwrap_or("").to_string();
    if tag.is_empty() || !is_newer(&tag, VERSION) {
        return Ok(None);
    }
    let installer = v["assets"].as_array().and_then(|assets| {
        assets.iter().find_map(|a| {
            let name = a["name"].as_str()?.to_ascii_lowercase();
            (name.ends_with("setup.exe")).then(|| a["browser_download_url"].as_str().map(str::to_string))?
        })
    });
    Ok(Some(Release {
        version: tag.trim_start_matches('v').to_string(),
        notes: v["body"].as_str().unwrap_or("").to_string(),
        page: v["html_url"].as_str().unwrap_or("").to_string(),
        installer,
    }))
}

fn download(url: &str, to: &Path, tx: &Sender<Msg>) -> Result<(), String> {
    let _ = tx.send(Msg::Log(format!("Downloading {url}")));
    let resp = agent()?.get(url).call().map_err(|e| e.to_string())?;
    let total: u64 = resp.header("Content-Length").and_then(|v| v.parse().ok()).unwrap_or(0);
    let mut reader = resp.into_reader();
    let mut file = std::fs::File::create(to).map_err(|e| e.to_string())?;
    let mut buf = vec![0u8; 1 << 16];
    let (mut done, mut last_pct) = (0u64, 0u64);
    loop {
        let n = reader.read(&mut buf).map_err(|e| e.to_string())?;
        if n == 0 {
            break;
        }
        std::io::Write::write_all(&mut file, &buf[..n]).map_err(|e| e.to_string())?;
        done += n as u64;
        if total > 0 {
            let pct = done * 100 / total;
            if pct >= last_pct + 10 {
                last_pct = pct;
                let _ = tx.send(Msg::Log(format!("  {pct}%  ({:.1} MB)", done as f64 / 1_048_576.0)));
            }
        }
    }
    Ok(())
}

fn exe_name(name: &str) -> String {
    if cfg!(windows) {
        format!("{name}.exe")
    } else {
        name.to_string()
    }
}

/// Finds Cargo, installing Rust with rustup when it is missing.
fn ensure_rust(tx: &Sender<Msg>) -> Result<PathBuf, String> {
    let home_cargo = std::env::var_os("CARGO_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("USERPROFILE").or_else(|| std::env::var_os("HOME")).map(|h| PathBuf::from(h).join(".cargo")))
        .map(|d| d.join("bin").join(exe_name("cargo")));
    let works = |p: &Path| Command::new(p).arg("--version").stdout(Stdio::null()).stderr(Stdio::null()).status().is_ok_and(|s| s.success());
    if works(Path::new("cargo")) {
        let _ = tx.send(Msg::Log("Rust is already installed (cargo found in PATH).".into()));
        return Ok(PathBuf::from("cargo"));
    }
    if let Some(p) = &home_cargo {
        if works(p) {
            let _ = tx.send(Msg::Log(format!("Rust is already installed ({}).", p.display())));
            return Ok(p.clone());
        }
    }
    let _ = tx.send(Msg::Log("Rust was not found — installing it with rustup (one time, a few minutes)…".into()));
    let tmp = std::env::temp_dir();
    let status = if cfg!(windows) {
        let init = tmp.join("rustup-init.exe");
        download("https://win.rustup.rs/x86_64", &init, tx)?;
        // The GNU host is self-contained: no Visual Studio Build Tools needed.
        Command::new(&init)
            .args(["-y", "--profile", "minimal", "--default-toolchain", "stable", "--default-host", "x86_64-pc-windows-gnu"])
            .status()
    } else {
        let script = tmp.join("rustup-init.sh");
        download("https://sh.rustup.rs", &script, tx)?;
        Command::new("sh").arg(&script).args(["-y", "--profile", "minimal"]).status()
    };
    match status {
        Ok(s) if s.success() => {}
        Ok(s) => return Err(format!("rustup failed ({s})")),
        Err(e) => return Err(format!("rustup could not start: {e}")),
    }
    let cargo = home_cargo.ok_or("Cannot locate the Cargo folder")?;
    if works(&cargo) {
        let _ = tx.send(Msg::Log("Rust installed.".into()));
        Ok(cargo)
    } else {
        Err("Rust was installed but cargo does not run".into())
    }
}

/// Runs a command, streaming its output into the log.
fn run_logged(mut cmd: Command, tx: &Sender<Msg>) -> Result<(), String> {
    let mut child = cmd.stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().map_err(|e| e.to_string())?;
    let stderr = child.stderr.take();
    let tx2 = tx.clone();
    let t = std::thread::spawn(move || {
        if let Some(s) = stderr {
            for line in std::io::BufReader::new(s).lines().map_while(Result::ok) {
                let _ = tx2.send(Msg::Log(line));
            }
        }
    });
    if let Some(s) = child.stdout.take() {
        for line in std::io::BufReader::new(s).lines().map_while(Result::ok) {
            let _ = tx.send(Msg::Log(line));
        }
    }
    let _ = t.join();
    let status = child.wait().map_err(|e| e.to_string())?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("command failed ({status})"))
    }
}

fn extract_zip(zip_path: &Path, dest: &Path) -> Result<PathBuf, String> {
    let f = std::fs::File::open(zip_path).map_err(|e| e.to_string())?;
    let mut zip = zip::ZipArchive::new(f).map_err(|e| e.to_string())?;
    zip.extract(dest).map_err(|e| e.to_string())?;
    // GitHub archives contain a single top-level folder.
    std::fs::read_dir(dest)
        .map_err(|e| e.to_string())?
        .flatten()
        .map(|e| e.path())
        .find(|p| p.join("Cargo.toml").exists())
        .ok_or_else(|| "Source archive has no Cargo.toml".into())
}

fn build_from_source(tx: &Sender<Msg>) -> Result<String, String> {
    let cargo = ensure_rust(tx)?;
    let work = std::env::temp_dir().join("forge-design-update");
    let _ = std::fs::create_dir_all(&work);
    let zip_path = work.join("main.zip");
    download(&format!("https://github.com/{REPO}/archive/refs/heads/main.zip"), &zip_path, tx)?;
    let src_parent = work.join("src");
    if src_parent.exists() {
        let _ = std::fs::remove_dir_all(&src_parent);
    }
    let src = extract_zip(&zip_path, &src_parent)?;
    let _ = tx.send(Msg::Log(format!("Building in {} (first build takes a few minutes)…", src.display())));
    let mut cmd = Command::new(&cargo);
    cmd.current_dir(&src).args(["build", "--release", "-p", "forge-app", "-p", "forge-cli"]);
    run_logged(cmd, tx)?;
    let built = src.join("target").join("release").join(exe_name("forge"));
    if !built.exists() {
        return Err("Build finished but forge executable is missing".into());
    }
    // Replace the running executable: rename the old one first (Windows
    // allows renaming a running .exe, but not overwriting it).
    let current = std::env::current_exe().map_err(|e| e.to_string())?;
    let dir = current.parent().ok_or("No install folder")?;
    let old = current.with_extension("old");
    let _ = std::fs::remove_file(&old);
    std::fs::rename(&current, &old).map_err(|e| format!("Cannot replace {}: {e} (try running as administrator)", current.display()))?;
    std::fs::copy(&built, &current).map_err(|e| e.to_string())?;
    let cli = src.join("target").join("release").join(exe_name("forge-cli"));
    if cli.exists() {
        let _ = std::fs::copy(&cli, dir.join(exe_name("forge-cli")));
    }
    // Bring new bundled fonts along.
    let fonts = src.join("assets").join("fonts");
    if fonts.exists() {
        let _ = std::fs::create_dir_all(dir.join("fonts"));
        if let Ok(rd) = std::fs::read_dir(&fonts) {
            for f in rd.flatten() {
                let _ = std::fs::copy(f.path(), dir.join("fonts").join(f.file_name()));
            }
        }
    }
    Ok("Updated from source. Restart Forge Design to use the new version.".into())
}

impl Updater {
    pub fn busy(&self) -> bool {
        matches!(self.state, State::Checking | State::Working)
    }

    fn spawn(&mut self, f: impl FnOnce(Sender<Msg>) + Send + 'static) {
        let (tx, rx) = channel();
        self.rx = Some(rx);
        std::thread::spawn(move || f(tx));
    }

    /// Checks GitHub for a newer release. `silent` = startup check.
    pub fn check(&mut self, silent: bool) {
        if self.busy() {
            return;
        }
        self.silent = silent;
        if !silent {
            self.open = true;
        }
        self.state = State::Checking;
        self.spawn(|tx| {
            let _ = tx.send(Msg::Checked(fetch_latest()));
        });
    }

    /// Downloads and runs the setup wizard of the available release.
    pub fn install_release(&mut self) {
        let Some(rel) = self.release.clone() else { return };
        let Some(url) = rel.installer.clone() else {
            open_url(&rel.page);
            return;
        };
        self.state = State::Working;
        self.log.clear();
        self.spawn(move |tx| {
            let path = std::env::temp_dir().join(format!("ForgeDesign-{}-Setup.exe", rel.version));
            let r = download(&url, &path, &tx).and_then(|_| {
                let _ = tx.send(Msg::Log("Starting the setup wizard…".into()));
                Command::new(&path).spawn().map(|_| "INSTALLER".to_string()).map_err(|e| e.to_string())
            });
            let _ = tx.send(Msg::Finished(r));
        });
    }

    /// Downloads the newest source from GitHub and builds it (installs Rust if needed).
    pub fn update_from_source(&mut self) {
        if self.busy() {
            return;
        }
        self.open = true;
        self.state = State::Working;
        self.log.clear();
        self.spawn(|tx| {
            let r = build_from_source(&tx);
            let _ = tx.send(Msg::Finished(r));
        });
    }

    /// Drains background messages. Returns `true` when the app should quit
    /// (the installer took over).
    pub fn poll(&mut self, ctx: &eframe::egui::Context) -> bool {
        let mut quit = false;
        let Some(rx) = &self.rx else { return false };
        while let Ok(m) = rx.try_recv() {
            match m {
                Msg::Log(l) => {
                    self.log.push(l);
                    if self.log.len() > 500 {
                        self.log.drain(..100);
                    }
                }
                Msg::Checked(Ok(Some(rel))) => {
                    self.release = Some(rel);
                    self.state = State::Available;
                    self.open = true;
                }
                Msg::Checked(Ok(None)) => self.state = State::UpToDate,
                Msg::Checked(Err(e)) => {
                    self.state = State::Failed(e);
                    if self.silent {
                        self.state = State::Idle;
                    }
                }
                Msg::Finished(Ok(s)) if s == "INSTALLER" => quit = true,
                Msg::Finished(Ok(s)) => self.state = State::Done(s),
                Msg::Finished(Err(e)) => self.state = State::Failed(e),
            }
        }
        if self.busy() {
            ctx.request_repaint_after(std::time::Duration::from_millis(200));
        }
        quit
    }

    /// The Updates window.
    pub fn ui(&mut self, ctx: &eframe::egui::Context) {
        use eframe::egui;
        if !self.open {
            return;
        }
        let mut open = self.open;
        egui::Window::new("Updates").open(&mut open).collapsible(false).default_width(460.0).show(ctx, |ui| {
            ui.label(format!("Installed version: {VERSION}"));
            ui.label(egui::RichText::new(format!("Source: github.com/{REPO}")).weak());
            ui.separator();
            match &self.state {
                State::Idle => {
                    ui.label("Check GitHub for a newer version.");
                }
                State::Checking => {
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.label("Checking GitHub…");
                    });
                }
                State::UpToDate => {
                    ui.label("✔ Forge Design is up to date (no newer release on GitHub).");
                }
                State::Available => {
                    if let Some(r) = &self.release {
                        ui.label(egui::RichText::new(format!("Version {} is available!", r.version)).strong().color(crate::app::ACCENT));
                        if !r.notes.is_empty() {
                            egui::ScrollArea::vertical().max_height(140.0).show(ui, |ui| {
                                ui.label(&r.notes);
                            });
                        }
                    }
                }
                State::Working => {
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.label("Working… you can keep editing.");
                    });
                }
                State::Done(s) => {
                    ui.label(egui::RichText::new(format!("✔ {s}")).color(crate::app::ACCENT));
                }
                State::Failed(e) => {
                    ui.colored_label(egui::Color32::LIGHT_RED, format!("✖ {e}"));
                }
            }
            ui.separator();
            ui.horizontal_wrapped(|ui| {
                if ui.add_enabled(!self.busy(), egui::Button::new("Check now")).clicked() {
                    self.check(false);
                }
                let has_release = matches!(self.state, State::Available);
                let label = if self.release.as_ref().is_some_and(|r| r.installer.is_some()) {
                    "Download & install"
                } else {
                    "Open release page"
                };
                if ui.add_enabled(has_release && !self.busy(), egui::Button::new(label)).clicked() {
                    self.install_release();
                }
                if ui
                    .add_enabled(!self.busy(), egui::Button::new("Update from source (main)"))
                    .on_hover_text("Downloads the newest code from GitHub and builds it. Installs Rust automatically if it is not installed.")
                    .clicked()
                {
                    self.update_from_source();
                }
            });
            if !self.log.is_empty() {
                ui.separator();
                egui::ScrollArea::vertical().max_height(180.0).stick_to_bottom(true).show(ui, |ui| {
                    for l in &self.log {
                        ui.label(egui::RichText::new(l).monospace().size(11.0));
                    }
                });
            }
        });
        self.open = open;
    }
}

pub fn open_url(url: &str) {
    if url.is_empty() {
        return;
    }
    #[cfg(windows)]
    let _ = Command::new("cmd").args(["/C", "start", "", url]).spawn();
    #[cfg(target_os = "macos")]
    let _ = Command::new("open").arg(url).spawn();
    #[cfg(all(unix, not(target_os = "macos")))]
    let _ = Command::new("xdg-open").arg(url).spawn();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions() {
        assert_eq!(parse_version("v1.2.3"), (1, 2, 3));
        assert_eq!(parse_version("0.10.0-beta"), (0, 10, 0));
        assert!(is_newer("v0.2.0", "0.1.9"));
        assert!(is_newer("0.1.10", "0.1.9"));
        assert!(!is_newer("v0.1.1", "0.1.1"));
        assert!(!is_newer("garbage", "0.1.0"));
    }
}
