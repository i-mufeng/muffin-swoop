//! 持久化列表布局；列宽始终跟随列标识，配置写入失败时不应用。
use std::collections::HashSet;

use serde::{Deserialize, Serialize};
use swoop_core::Store;

use crate::task_transfer::ListPreferences;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ListMode {
    #[default]
    Cards,
    Table,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ColumnId {
    Name,
    Status,
    Size,
    Progress,
    Speed,
    Remaining,
    Connections,
    Host,
    Directory,
    Created,
}
impl ColumnId {
    pub const ALL: [Self; 10] = [
        Self::Name,
        Self::Status,
        Self::Size,
        Self::Progress,
        Self::Speed,
        Self::Remaining,
        Self::Connections,
        Self::Host,
        Self::Directory,
        Self::Created,
    ];
    pub fn label(self) -> &'static str {
        match self {
            Self::Name => swoop_i18n::tr!("名称"),
            Self::Status => swoop_i18n::tr!("状态"),
            Self::Size => swoop_i18n::tr!("大小"),
            Self::Progress => swoop_i18n::tr!("进度"),
            Self::Speed => swoop_i18n::tr!("速度"),
            Self::Remaining => swoop_i18n::tr!("剩余时间"),
            Self::Connections => swoop_i18n::tr!("连接数"),
            Self::Host => swoop_i18n::tr!("来源站点"),
            Self::Directory => swoop_i18n::tr!("保存目录"),
            Self::Created => swoop_i18n::tr!("添加时间"),
        }
    }
    pub fn default_width(self) -> f32 {
        match self {
            Self::Name => 250.0,
            Self::Directory => 240.0,
            Self::Host => 170.0,
            Self::Created => 145.0,
            Self::Progress | Self::Connections => 80.0,
            _ => 110.0,
        }
    }
    pub fn min_width(self) -> f32 {
        if self == Self::Name { 160.0 } else { 64.0 }
    }
    fn default_spec(self) -> ColumnSpec {
        ColumnSpec {
            id: self,
            visible: matches!(
                self,
                Self::Name | Self::Status | Self::Size | Self::Progress | Self::Speed | Self::Remaining
            ),
            width: self.default_width(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ColumnSpec {
    pub id: ColumnId,
    pub visible: bool,
    pub width: f32,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ListLayout {
    pub version: u32,
    pub mode: ListMode,
    pub columns: Vec<ColumnSpec>,
}
impl Default for ListLayout {
    fn default() -> Self {
        Self {
            version: 1,
            mode: ListMode::Cards,
            columns: ColumnId::ALL.into_iter().map(ColumnId::default_spec).collect(),
        }
    }
}
impl ListLayout {
    pub fn normalize(&mut self) {
        if self.version != 1 {
            *self = Self::default();
            return;
        }
        let mut seen = HashSet::new();
        self.columns.retain(|column| seen.insert(column.id));
        for id in ColumnId::ALL {
            if !seen.contains(&id) {
                self.columns.push(id.default_spec());
            }
        }
        for column in &mut self.columns {
            if column.id == ColumnId::Name {
                column.visible = true;
            }
            column.width = if column.width.is_finite() {
                column.width.clamp(column.id.min_width(), 800.0)
            } else {
                column.id.default_width()
            };
        }
    }
    pub fn visible(&self) -> impl Iterator<Item = &ColumnSpec> {
        self.columns.iter().filter(|column| column.visible)
    }
    pub fn with_widths(&self, widths: &[(ColumnId, f32)]) -> Self {
        let mut next = self.clone();
        for column in &mut next.columns {
            if let Some((_, width)) = widths.iter().find(|(id, width)| *id == column.id && width.is_finite())
                && (column.width - width).abs() >= 0.5
            {
                column.width = *width;
            }
        }
        next.normalize();
        next
    }
    fn from_legacy(view: ListPreferences) -> Self {
        let mut layout = Self::default();
        for column in &mut layout.columns {
            column.visible = match column.id {
                ColumnId::Name | ColumnId::Status => true,
                ColumnId::Size => view.size,
                ColumnId::Progress => view.percent,
                ColumnId::Speed => view.speed,
                ColumnId::Remaining => view.remaining_time,
                ColumnId::Connections => view.connections,
                ColumnId::Host => view.host,
                ColumnId::Directory => view.directory,
                ColumnId::Created => false,
            };
        }
        layout
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolbarAction {
    New,
    ResumeAll,
    PauseAll,
    Import,
    Media,
    Grabber,
}
impl ToolbarAction {
    pub const ALL: [Self; 6] = [Self::New, Self::ResumeAll, Self::PauseAll, Self::Import, Self::Media, Self::Grabber];
    pub fn label(self) -> &'static str {
        match self {
            Self::New => swoop_i18n::tr!("新建任务"),
            Self::ResumeAll => swoop_i18n::tr!("全部开始"),
            Self::PauseAll => swoop_i18n::tr!("全部暂停"),
            Self::Import => swoop_i18n::tr!("导入链接"),
            Self::Media => swoop_i18n::tr!("媒体下载"),
            Self::Grabber => swoop_i18n::tr!("站点抓取"),
        }
    }
    pub fn icon(self) -> &'static str {
        use egui_phosphor::regular as i;
        match self {
            Self::New => i::PLUS,
            Self::ResumeAll => i::PLAY,
            Self::PauseAll => i::PAUSE,
            Self::Import => i::FILE_ARROW_UP,
            Self::Media => i::FILM_STRIP,
            Self::Grabber => i::GLOBE,
        }
    }
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ToolbarLayout {
    pub order: Vec<ToolbarAction>,
    pub hidden: Vec<ToolbarAction>,
}
impl Default for ToolbarLayout {
    fn default() -> Self {
        Self {
            order: ToolbarAction::ALL.into(),
            hidden: vec![ToolbarAction::Import, ToolbarAction::Media, ToolbarAction::Grabber],
        }
    }
}
impl ToolbarLayout {
    pub fn normalize(&mut self) {
        let mut seen = HashSet::new();
        self.order.retain(|action| seen.insert(*action));
        for action in ToolbarAction::ALL {
            if !seen.contains(&action) {
                self.order.push(action);
            }
        }
        seen.clear();
        self.hidden.retain(|action| seen.insert(*action));
    }
    pub fn visible(&self) -> impl DoubleEndedIterator<Item = ToolbarAction> + '_ {
        self.order.iter().copied().filter(|action| !self.hidden.contains(action))
    }
    fn from_legacy(view: ListPreferences) -> Self {
        let mut layout = Self::default();
        if !view.toolbar_actions {
            layout.hidden.extend([ToolbarAction::ResumeAll, ToolbarAction::PauseAll]);
        }
        layout
    }
}

pub struct LayoutPreferences {
    pub list: ListLayout,
    pub toolbar: ToolbarLayout,
    pub revision: u64,
}
impl LayoutPreferences {
    pub fn load(store: &Store, legacy: ListPreferences) -> Self {
        let mut list: ListLayout = store
            .get_setting("task_list_layout")
            .ok()
            .flatten()
            .and_then(|v| serde_json::from_str(&v).ok())
            .unwrap_or_else(|| ListLayout::from_legacy(legacy));
        let mut toolbar: ToolbarLayout = store
            .get_setting("task_toolbar_layout")
            .ok()
            .flatten()
            .and_then(|v| serde_json::from_str(&v).ok())
            .unwrap_or_else(|| ToolbarLayout::from_legacy(legacy));
        list.normalize();
        toolbar.normalize();
        Self { list, toolbar, revision: 0 }
    }
    pub fn save(&mut self, store: &Store, mut list: ListLayout, mut toolbar: ToolbarLayout) -> Result<(), String> {
        list.normalize();
        toolbar.normalize();
        let pairs = [
            ("task_list_layout", serde_json::to_string(&list).map_err(|e| e.to_string())?),
            ("task_toolbar_layout", serde_json::to_string(&toolbar).map_err(|e| e.to_string())?),
        ];
        // Reset egui's index-based width cache on reorder, explicit width edit, or a failed resize save.
        self.revision = self.revision.wrapping_add(1);
        store.set_settings(&pairs).map_err(|e| e.to_string())?;
        self.list = list;
        self.toolbar = toolbar;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn old_json_upgrades_without_changing_cards_or_rewriting_legacy_settings() {
        let store = Store::in_memory().unwrap();
        let old = r#"{"compact":true,"host":true,"size":false,"toolbar_actions":false}"#;
        store.set_setting("task_list_view", old).unwrap();
        let legacy = serde_json::from_str(old).unwrap();
        let mut layout = LayoutPreferences::load(&store, legacy);
        assert_eq!(layout.list.mode, ListMode::Cards);
        assert!(!layout.list.columns.iter().find(|c| c.id == ColumnId::Size).unwrap().visible);
        assert!(layout.list.columns.iter().find(|c| c.id == ColumnId::Host).unwrap().visible);
        assert_eq!(layout.toolbar.visible().collect::<Vec<_>>(), [ToolbarAction::New]);
        layout.save(&store, layout.list.clone(), layout.toolbar.clone()).unwrap();
        assert_eq!(store.get_setting("task_list_view").unwrap().unwrap(), old);
        assert_eq!(LayoutPreferences::load(&store, ListPreferences::default()).list, layout.list);
    }
    #[test]
    fn invalid_columns_are_bounded_deduplicated_and_name_remains_visible() {
        let mut layout = ListLayout {
            columns: vec![
                ColumnSpec { id: ColumnId::Name, visible: false, width: f32::NAN },
                ColumnSpec { id: ColumnId::Name, visible: true, width: 600.0 },
                ColumnSpec { id: ColumnId::Size, visible: true, width: -20.0 },
                ColumnSpec { id: ColumnId::Host, visible: true, width: 5000.0 },
            ],
            ..Default::default()
        };
        layout.normalize();
        assert_eq!(layout.columns.len(), 10);
        assert!(layout.columns[0].visible);
        assert_eq!(layout.columns[0].width, ColumnId::Name.default_width());
        assert_eq!(layout.columns[1].width, ColumnId::Size.min_width());
        assert_eq!(layout.columns[2].width, 800.0);
        let store = Store::in_memory().unwrap();
        for invalid in [
            "broken",
            r#"{"mode":"unknown"}"#,
            r#"{"version":999}"#,
            r#"{"columns":[{"id":"execute_shell","visible":true,"width":100}]}"#,
        ] {
            store.set_setting("task_list_layout", invalid).unwrap();
            let loaded = LayoutPreferences::load(&store, ListPreferences::default());
            assert_eq!(loaded.list.mode, ListMode::Cards);
            assert_eq!(loaded.list.columns.len(), 10);
        }
        let mut toolbar = ToolbarLayout {
            order: vec![ToolbarAction::Media, ToolbarAction::Media],
            hidden: vec![ToolbarAction::New, ToolbarAction::New],
        };
        toolbar.normalize();
        assert_eq!(toolbar.order.len(), 6);
        assert_eq!(toolbar.hidden, [ToolbarAction::New]);
    }
    #[test]
    fn widths_follow_column_ids_after_reorder_and_hidden_columns_keep_width() {
        let mut layout = ListLayout::default();
        layout.columns.swap(0, 2);
        let resized = layout.with_widths(&[(ColumnId::Name, 321.0), (ColumnId::Size, 156.0)]);
        assert_eq!(resized.columns[0].id, ColumnId::Size);
        assert_eq!(resized.columns[0].width, 156.0);
        assert_eq!(resized.columns[2].id, ColumnId::Name);
        assert_eq!(resized.columns[2].width, 321.0);
        assert_eq!(resized.columns[7].width, layout.columns[7].width);
    }
    #[test]
    fn sqlite_failure_rolls_back_both_settings_and_in_memory_layout() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("layout.db");
        let store = Store::open(&path).unwrap();
        let mut prefs = LayoutPreferences::load(&store, ListPreferences::default());
        prefs.save(&store, prefs.list.clone(), prefs.toolbar.clone()).unwrap();
        let before = prefs.list.clone();
        let toolbar_before = prefs.toolbar.clone();
        let conn = rusqlite::Connection::open(&path).unwrap();
        conn.execute_batch("CREATE TRIGGER reject_toolbar BEFORE INSERT ON settings WHEN NEW.key='task_toolbar_layout' BEGIN SELECT RAISE(FAIL, 'test layout write rejected'); END;").unwrap();
        let mut changed = before.clone();
        changed.mode = ListMode::Table;
        changed.columns.swap(0, 1);
        let mut toolbar = toolbar_before.clone();
        toolbar.hidden.push(ToolbarAction::New);
        assert!(
            prefs.save(&store, changed.clone(), toolbar.clone()).unwrap_err().contains("test layout write rejected")
        );
        assert_eq!(prefs.list, before);
        assert_eq!(prefs.toolbar, toolbar_before);
        let loaded = LayoutPreferences::load(&store, ListPreferences::default());
        assert_eq!(loaded.list, before);
        assert_eq!(loaded.toolbar, toolbar_before);
        conn.execute_batch("DROP TRIGGER reject_toolbar").unwrap();
        prefs.save(&store, changed.clone(), toolbar.clone()).unwrap();
        drop(store);
        let reopened = Store::open(&path).unwrap();
        let loaded = LayoutPreferences::load(&reopened, ListPreferences::default());
        assert_eq!(loaded.list, changed);
        assert_eq!(loaded.toolbar, toolbar);
    }
}
