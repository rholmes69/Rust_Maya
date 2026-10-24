//! Preferences (UI layout, recent files, tool options) and autosave / crash recovery.

use crate::app::{App, MenuSet, RightTab, Shading, Workspace};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

pub const MAX_RECENT: usize = 10;
const AUTOSAVE_KEEP: usize = 3;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Prefs {
    pub recent: Vec<PathBuf>,
    pub autosave_enabled: bool,
    pub autosave_minutes: f32,
    /// False while the app runs; set on a clean exit. If false at startup the last session crashed.
    pub clean_exit: bool,
    pub outliner: bool,
    pub right: bool,
    pub shelf: bool,
    pub toolbox: bool,
    pub timeline: bool,
    pub range: bool,
    pub cmdline: bool,
    pub helpline: bool,
    pub status: bool,
    pub copilot_open: bool,
    pub workspace: Workspace,
    pub menu_set: MenuSet,
    pub right_tab: RightTab,
    pub shelf_tab: usize,
    pub quad: bool,
    pub shading: [Shading; 4],
    pub show_grid: [bool; 4],
    pub snap_grid: bool,
    pub joint_radius: f32,
    pub extrude_offset: f32,
    pub auto_key: bool,
    pub loop_playback: bool,
    pub load_demo_scene: bool,
}

impl Default for Prefs {
    fn default() -> Self {
        Prefs {
            recent: Vec::new(),
            autosave_enabled: true,
            autosave_minutes: 5.0,
            clean_exit: true,
            outliner: true,
            right: true,
            shelf: true,
            toolbox: true,
            timeline: true,
            range: true,
            cmdline: true,
            helpline: true,
            status: true,
            copilot_open: true,
            workspace: Workspace::MayaClassic,
            menu_set: MenuSet::Modeling,
            right_tab: RightTab::ChannelBox,
            shelf_tab: 1,
            quad: false,
            shading: [Shading::Shaded, Shading::Wireframe, Shading::Wireframe, Shading::Wireframe],
            show_grid: [true; 4],
            snap_grid: false,
            joint_radius: 0.5,
            extrude_offset: 0.25,
            auto_key: false,
            loop_playback: true,
            load_demo_scene: true,
        }
    }
}

pub fn config_dir() -> PathBuf {
    dirs::config_dir().unwrap_or_else(std::env::temp_dir).join("rust_maya")
}

pub fn autosave_dir() -> PathBuf {
    config_dir().join("autosave")
}

impl Prefs {
    pub fn path() -> PathBuf {
        config_dir().join("prefs.json")
    }

    pub fn load() -> Self {
        std::fs::read_to_string(Self::path()).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default()
    }

    pub fn save(&self) {
        let _ = std::fs::create_dir_all(config_dir());
        if let Ok(s) = serde_json::to_string_pretty(self) {
            let _ = std::fs::write(Self::path(), s);
        }
    }

    /// Apply stored layout/options to a fresh App.
    pub fn apply(&self, app: &mut App) {
        let p = &mut app.panels;
        p.outliner = self.outliner;
        p.right = self.right;
        p.shelf = self.shelf;
        p.toolbox = self.toolbox;
        p.timeline = self.timeline;
        p.range = self.range;
        p.cmdline = self.cmdline;
        p.helpline = self.helpline;
        p.status = self.status;
        app.workspace = self.workspace;
        app.menu_set = self.menu_set;
        app.right_tab = self.right_tab;
        app.shelf_tab = self.shelf_tab;
        app.quad = self.quad;
        for i in 0..4 {
            app.views[i].shading = self.shading[i];
            app.views[i].show_grid = self.show_grid[i];
        }
        app.snap_grid = self.snap_grid;
        app.joint_radius = self.joint_radius;
        app.extrude_offset = self.extrude_offset;
        app.auto_key = self.auto_key;
        app.loop_playback = self.loop_playback;
        app.recent_files = self.recent.iter().filter(|p| p.exists()).cloned().collect();
    }

    /// Capture the current layout/options.
    pub fn capture(&mut self, app: &App, copilot_open: bool) {
        let p = &app.panels;
        self.outliner = p.outliner;
        self.right = p.right;
        self.shelf = p.shelf;
        self.toolbox = p.toolbox;
        self.timeline = p.timeline;
        self.range = p.range;
        self.cmdline = p.cmdline;
        self.helpline = p.helpline;
        self.status = p.status;
        self.copilot_open = copilot_open;
        self.workspace = app.workspace;
        self.menu_set = app.menu_set;
        self.right_tab = app.right_tab;
        self.shelf_tab = app.shelf_tab;
        self.quad = app.quad;
        for i in 0..4 {
            self.shading[i] = app.views[i].shading;
            self.show_grid[i] = app.views[i].show_grid;
        }
        self.snap_grid = app.snap_grid;
        self.joint_radius = app.joint_radius;
        self.extrude_offset = app.extrude_offset;
        self.auto_key = app.auto_key;
        self.loop_playback = app.loop_playback;
        self.recent = app.recent_files.clone();
    }
}

/// Add a path to the front of a recent-files list.
pub fn push_recent(list: &mut Vec<PathBuf>, p: &Path) {
    list.retain(|x| x != p);
    list.insert(0, p.to_path_buf());
    list.truncate(MAX_RECENT);
}

/// Write an autosave of the scene; keeps the newest few files. Returns the file written.
pub fn autosave(app: &App) -> Result<PathBuf, String> {
    let dir = autosave_dir();
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let stem = app.file_path.as_ref().and_then(|p| p.file_stem()).map(|s| s.to_string_lossy().to_string()).unwrap_or_else(|| "untitled".into());
    let stamp = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    let path = dir.join(format!("{stem}_autosave_{stamp}.rma"));
    let json = serde_json::to_string(&app.scene).map_err(|e| e.to_string())?;
    std::fs::write(&path, json).map_err(|e| e.to_string())?;
    // Prune old autosaves.
    let mut all = list_autosaves();
    while all.len() > AUTOSAVE_KEEP {
        if let Some((p, _)) = all.pop() {
            let _ = std::fs::remove_file(p);
        }
    }
    Ok(path)
}

/// Autosave files, newest first, with their modification times.
pub fn list_autosaves() -> Vec<(PathBuf, SystemTime)> {
    let mut v: Vec<(PathBuf, SystemTime)> = std::fs::read_dir(autosave_dir())
        .map(|rd| {
            rd.filter_map(|e| e.ok())
                .filter(|e| e.path().extension().is_some_and(|x| x == "rma"))
                .filter_map(|e| Some((e.path(), e.metadata().ok()?.modified().ok()?)))
                .collect()
        })
        .unwrap_or_default();
    v.sort_by(|a, b| b.1.cmp(&a.1));
    v
}

pub fn age_text(t: SystemTime) -> String {
    let secs = SystemTime::now().duration_since(t).map(|d| d.as_secs()).unwrap_or(0);
    match secs {
        0..=59 => format!("{secs} s ago"),
        60..=3599 => format!("{} min ago", secs / 60),
        3600..=86_399 => format!("{} h ago", secs / 3600),
        _ => format!("{} days ago", secs / 86_400),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recent_list_dedupes_and_caps() {
        let mut v = Vec::new();
        for i in 0..15 {
            push_recent(&mut v, Path::new(&format!("/tmp/s{i}.rma")));
        }
        push_recent(&mut v, Path::new("/tmp/s3.rma"));
        assert_eq!(v.len(), MAX_RECENT);
        assert_eq!(v[0], PathBuf::from("/tmp/s3.rma"));
        assert_eq!(v.iter().filter(|p| **p == PathBuf::from("/tmp/s3.rma")).count(), 1);
    }

    #[test]
    fn prefs_roundtrip() {
        let mut app = App::new();
        app.quad = true;
        app.panels.outliner = false;
        app.views[0].shading = Shading::WireOnShaded;
        let mut p = Prefs::default();
        p.capture(&app, false);
        let json = serde_json::to_string(&p).unwrap();
        let back: Prefs = serde_json::from_str(&json).unwrap();
        let mut app2 = App::new();
        back.apply(&mut app2);
        assert!(app2.quad && !app2.panels.outliner && !back.copilot_open);
        assert_eq!(app2.views[0].shading, Shading::WireOnShaded);
        // Unknown/missing fields fall back to defaults.
        let partial: Prefs = serde_json::from_str("{\"quad\":true}").unwrap();
        assert!(partial.autosave_enabled && partial.quad);
    }
}
