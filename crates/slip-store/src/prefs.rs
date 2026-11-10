//! 展示偏好（v11 §3.5）。
//!
//! 偏好只影响展示，**不影响 `content_rev`**，失败不改变 Markdown 的 ACK。

use std::path::{Path, PathBuf};

use slip_domain::SlipId;

use crate::fsops;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum DockSide {
    #[default]
    Left,
    Right,
    Top,
    Bottom,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Prefs {
    pub order: Vec<SlipId>,
    pub collapsed: Vec<SlipId>,
    pub dock_side: DockSide,
    /// 全局热键，例如 `ctrl+alt+space`；`None` 表示用外壳的默认值。
    ///
    /// 刻意存成**不透明字符串**：键名的文法属于 GUI 外壳（`tauri-plugin-global-shortcut`），
    /// 存储层不该认识它。合法性判定与回落默认值都由外壳负责，这里只负责原样存取。
    ///
    /// 它严格说不是"展示偏好"，但为它单开一个配置文件不值当；损坏时的处理
    /// 语义也完全一致（回默认值、不阻塞 Markdown）。
    #[serde(default)]
    pub hotkey: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PrefsError {
    Io(crate::IoKind),
    Corrupt,
}

pub trait PrefsStore: Send + Sync {
    fn load_prefs(&self) -> Prefs;
    fn save_prefs(&self, prefs: &Prefs) -> Result<(), PrefsError>;
}

/// `ui-state.json` 的 [`PrefsStore`] 实现。
///
/// 三条边界（v11 §3.5）：
///
/// - 保存走同目录 tmp + 原子替换，**不 fsync**——偏好丢了不伤内容，不值得为它付 fsync 代价；
/// - 缺失或损坏只影响展示：一律回默认值，**不阻塞 Markdown**；
/// - 损坏时尝试留一份 `ui-state.json.bad-<unix_ms>` 证据，重名不覆盖；留档失败也照常回默认值。
pub struct JsonPrefsStore {
    path: PathBuf,
}

impl JsonPrefsStore {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// 把损坏的偏好文件移到一边。失败无所谓：偏好不是真源。
    fn park_corrupt(&self) {
        if let Err(kind) = fsops::move_aside(&self.path, "bad") {
            tracing::warn!(?kind, path = %self.path.display(), "隔离损坏的偏好文件失败");
        }
    }
}

impl PrefsStore for JsonPrefsStore {
    fn load_prefs(&self) -> Prefs {
        // `load_prefs` 只在初始化调用一次；不反复读、不重复隔离坏文件。
        match fsops::read_bytes(&self.path) {
            Ok(bytes) => match serde_json::from_slice::<Prefs>(&bytes) {
                Ok(prefs) => prefs,
                Err(error) => {
                    tracing::warn!(%error, path = %self.path.display(), "偏好文件损坏，回默认值");
                    self.park_corrupt();
                    Prefs::default()
                }
            },
            // 缺失或读不到：默认偏好。
            Err(_) => Prefs::default(),
        }
    }

    fn save_prefs(&self, prefs: &Prefs) -> Result<(), PrefsError> {
        let bytes = serde_json::to_vec(prefs).map_err(|error| {
            tracing::warn!(%error, "偏好序列化失败");
            PrefsError::Io(crate::IoKind::Other)
        })?;
        fsops::replace_without_sync(&self.path, &bytes).map_err(PrefsError::Io)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_prefs() -> Prefs {
        Prefs {
            order: vec![SlipId::new_v7(), SlipId::new_v7()],
            collapsed: vec![SlipId::new_v7()],
            dock_side: DockSide::Bottom,
            hotkey: Some(String::from("ctrl+alt+space")),
        }
    }

    #[test]
    fn prefs_written_before_hotkey_existed_still_load() {
        // 旧版本的 prefs.json 没有 hotkey 字段，不能因为多了个字段就让用户偏好全丢。
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("ui-state.json");
        std::fs::write(&path, br#"{"order":[],"collapsed":[],"dockSide":"left"}"#)
            .expect("write legacy prefs");
        let store = JsonPrefsStore::new(&path);

        let prefs = store.load_prefs();
        assert_eq!(prefs, Prefs::default());
        assert_eq!(prefs.hotkey, None);
    }

    #[test]
    fn hotkey_survives_a_roundtrip_unparsed() {
        // 存储层不认识热键文法，也就不该改写它：原样进、原样出。
        let dir = tempfile::tempdir().expect("temp dir");
        let store = JsonPrefsStore::new(dir.path().join("ui-state.json"));
        let weird = String::from("Ctrl+Shift+Alt+F12");

        let prefs = Prefs {
            hotkey: Some(weird.clone()),
            ..Prefs::default()
        };
        store.save_prefs(&prefs).expect("save");

        assert_eq!(store.load_prefs().hotkey, Some(weird));
    }

    #[test]
    fn prefs_roundtrip() {
        let dir = tempfile::tempdir().expect("temp dir");
        let store = JsonPrefsStore::new(dir.path().join("ui-state.json"));
        let prefs = sample_prefs();

        store.save_prefs(&prefs).expect("save");

        assert_eq!(store.load_prefs(), prefs);
    }

    #[test]
    fn missing_file_returns_default() {
        let dir = tempfile::tempdir().expect("temp dir");
        let store = JsonPrefsStore::new(dir.path().join("ui-state.json"));

        assert_eq!(store.load_prefs(), Prefs::default());
    }

    #[test]
    fn corrupt_file_returns_default_and_keeps_bad_copy() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("ui-state.json");
        std::fs::write(&path, b"{not json").expect("break prefs");
        let store = JsonPrefsStore::new(&path);

        assert_eq!(store.load_prefs(), Prefs::default());

        let mut parked: Vec<String> = std::fs::read_dir(dir.path())
            .expect("list")
            .map(|entry| {
                entry
                    .expect("entry")
                    .file_name()
                    .to_string_lossy()
                    .into_owned()
            })
            .collect();
        parked.sort();
        assert_eq!(parked.len(), 1);
        assert!(
            parked[0].starts_with("ui-state.json.bad-"),
            "损坏文件应留档为 .bad-<ts>，实际 {}",
            parked[0]
        );
        assert!(!path.exists(), "损坏文件应已被移走");
        assert_eq!(
            std::fs::read(dir.path().join(&parked[0])).expect("read parked"),
            b"{not json".to_vec()
        );
    }

    #[test]
    fn save_replaces_atomically_without_leftover_temp_files() {
        let dir = tempfile::tempdir().expect("temp dir");
        let store = JsonPrefsStore::new(dir.path().join("ui-state.json"));
        store.save_prefs(&Prefs::default()).expect("save");
        store.save_prefs(&sample_prefs()).expect("save again");

        let names: Vec<String> = std::fs::read_dir(dir.path())
            .expect("list")
            .map(|entry| {
                entry
                    .expect("entry")
                    .file_name()
                    .to_string_lossy()
                    .into_owned()
            })
            .collect();
        assert_eq!(names, vec!["ui-state.json".to_owned()]);
    }

    #[test]
    fn duplicate_load_does_not_isolate_a_second_time() {
        // 第二次 load 时原文件已被移走，不再产生新的 .bad 副本。
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("ui-state.json");
        std::fs::write(&path, b"{not json").expect("break prefs");
        let store = JsonPrefsStore::new(&path);

        assert_eq!(store.load_prefs(), Prefs::default());
        assert_eq!(store.load_prefs(), Prefs::default());

        let count = std::fs::read_dir(dir.path()).expect("list").count();
        assert_eq!(count, 1);
    }
}
