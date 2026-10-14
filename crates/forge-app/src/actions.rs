//! Actions (record / play back sequences of commands) and Batch processing.

use std::path::{Path, PathBuf};

use forge_core::adjust::Adjustment;
use forge_core::filters::Filter;
use serde::{Deserialize, Serialize};

use crate::commands::{apply_adjustment_now, apply_filter_now, Cmd};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum ActionStep {
    Cmd(Cmd),
    Adjust(Adjustment),
    Filter(Filter),
}

impl ActionStep {
    pub fn label(&self) -> String {
        match self {
            ActionStep::Cmd(c) => format!("{c:?}"),
            ActionStep::Adjust(a) => a.name().to_string(),
            ActionStep::Filter(f) => f.name().to_string(),
        }
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Action {
    pub name: String,
    pub steps: Vec<ActionStep>,
}

#[derive(Default)]
pub struct Recorder {
    pub recording: bool,
    pub actions: Vec<Action>,
    pub selected: usize,
}

fn store_path() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("APPDATA").map(PathBuf::from))
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))?;
    Some(base.join("forge-design").join("actions.json"))
}

impl Recorder {
    pub fn load() -> Self {
        let actions = store_path()
            .and_then(|p| std::fs::read_to_string(p).ok())
            .and_then(|s| serde_json::from_str::<Vec<Action>>(&s).ok())
            .unwrap_or_else(default_actions);
        Self { recording: false, actions, selected: 0 }
    }

    pub fn save(&self) {
        if let Some(p) = store_path() {
            if let Some(dir) = p.parent() {
                let _ = std::fs::create_dir_all(dir);
            }
            if let Ok(s) = serde_json::to_string_pretty(&self.actions) {
                let _ = std::fs::write(p, s);
            }
        }
    }

    pub fn push(&mut self, step: ActionStep) {
        if let Some(a) = self.actions.get_mut(self.selected) {
            a.steps.push(step);
        }
    }

    pub fn start_new(&mut self) {
        let n = self.actions.len() + 1;
        self.actions.push(Action { name: format!("Action {n}"), steps: Vec::new() });
        self.selected = self.actions.len() - 1;
        self.recording = true;
    }

    pub fn stop(&mut self) {
        if self.recording {
            self.recording = false;
            self.save();
        }
    }
}

fn default_actions() -> Vec<Action> {
    vec![
        Action {
            name: "Vintage Look".into(),
            steps: vec![
                ActionStep::Adjust(Adjustment::HueSaturation { hue: 0.0, saturation: -35.0, lightness: 0.0, colorize: false }),
                ActionStep::Adjust(Adjustment::PhotoFilter { color: forge_core::Color::rgb(0.93, 0.54, 0.1), density: 0.35 }),
                ActionStep::Filter(Filter::LensCorrection { distortion: 0.0, vignette: -45.0, chromatic: 0.0 }),
                ActionStep::Filter(Filter::AddNoise { amount: 0.05, monochrome: true, seed: 4 }),
            ],
        },
        Action {
            name: "Web Sharpen".into(),
            steps: vec![ActionStep::Cmd(Cmd::AutoTone), ActionStep::Filter(Filter::SmartSharpen { amount: 0.8, radius: 0.8, noise: 5.0 })],
        },
    ]
}

/// Replays an action on the active document.
pub fn play(app: &mut crate::app::ForgeApp, action: &Action, ctx: &eframe::egui::Context) {
    let was = app.recorder.recording;
    app.recorder.recording = false;
    for step in &action.steps {
        match step {
            ActionStep::Cmd(c) => app.run(*c, ctx),
            ActionStep::Adjust(a) => {
                if let Some(tab) = app.tabs.get_mut(app.active) {
                    apply_adjustment_now(tab, a);
                }
            }
            ActionStep::Filter(f) => {
                if let Some(tab) = app.tabs.get_mut(app.active) {
                    apply_filter_now(tab, f);
                }
            }
        }
    }
    app.recorder.recording = was;
}

/// File > Automate > Batch: runs an action over every image in a folder.
pub fn batch(app: &mut crate::app::ForgeApp, action: &Action, input: &Path, output: &Path, ext: &str, ctx: &eframe::egui::Context) -> (usize, Vec<String>) {
    let mut done = 0;
    let mut errors = Vec::new();
    let Ok(rd) = std::fs::read_dir(input) else { return (0, vec![format!("Cannot read {}", input.display())]) };
    let mut files: Vec<PathBuf> = rd.flatten().map(|e| e.path()).filter(|p| p.is_file()).collect();
    files.sort();
    let _ = std::fs::create_dir_all(output);
    for f in files {
        let fext = f.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();
        if !forge_io::OPEN_EXTENSIONS.contains(&fext.as_str()) {
            continue;
        }
        match forge_io::open(&f) {
            Ok(doc) => {
                let prev_active = app.active;
                app.tabs.push(crate::canvas::DocTab::new(doc));
                app.active = app.tabs.len() - 1;
                play(app, action, ctx);
                let tab = app.tabs.pop().unwrap();
                app.active = prev_active.min(app.tabs.len().saturating_sub(1));
                let stem = f.file_stem().and_then(|s| s.to_str()).unwrap_or("image");
                let out = output.join(format!("{stem}.{ext}"));
                let r = if ext == "ora" || ext == "psd" {
                    forge_io::save(&tab.doc.state, &out)
                } else {
                    forge_io::export(&tab.doc.state, &out, forge_io::ExportOptions::default())
                };
                match r {
                    Ok(()) => done += 1,
                    Err(e) => errors.push(format!("{}: {e}", f.display())),
                }
            }
            Err(e) => errors.push(format!("{}: {e}", f.display())),
        }
    }
    (done, errors)
}
