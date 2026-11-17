//! App-owned menu labels follow the saved UI preference. Native edit/window
//! actions retain AppKit's responder behavior and standard keyboard shortcuts.
use crate::native_locale::Locale;
use tauri::{
    menu::{AboutMetadata, Menu, PredefinedMenuItem, Submenu, HELP_SUBMENU_ID, WINDOW_SUBMENU_ID},
    AppHandle, Wry,
};

enum Item {
    Action(PredefinedMenuItem<Wry>),
    Submenu(Submenu<Wry>),
}

pub(crate) struct Labels(Vec<(Item, [&'static str; 2])>);

impl Labels {
    pub fn apply(&self, locale: Locale) -> tauri::Result<()> {
        for (item, [zh, en]) in &self.0 {
            let text = locale.text(zh, en);
            match item {
                Item::Action(item) => item.set_text(text)?,
                Item::Submenu(item) => item.set_text(text)?,
            }
        }
        Ok(())
    }
}

pub(crate) fn install(app: &AppHandle, locale: Locale) -> tauri::Result<Labels> {
    let mut labels = Vec::new();
    macro_rules! action {
        ($method:ident, $zh:literal, $en:literal) => {{
            let item = PredefinedMenuItem::$method(app, Some(locale.text($zh, $en)))?;
            labels.push((Item::Action(item.clone()), [$zh, $en]));
            item
        }};
    }
    macro_rules! submenu {
        ($id:expr, $zh:literal, $en:literal, $items:expr) => {{
            let item = Submenu::with_id_and_items(app, $id, locale.text($zh, $en), true, $items)?;
            labels.push((Item::Submenu(item.clone()), [$zh, $en]));
            item
        }};
    }
    let about = PredefinedMenuItem::about(
        app,
        Some(locale.text("关于 SpeakMore", "About SpeakMore")),
        Some(AboutMetadata {
            name: Some("SpeakMore".into()),
            version: Some(app.package_info().version.to_string()),
            ..Default::default()
        }),
    )?;
    labels.push((
        Item::Action(about.clone()),
        ["关于 SpeakMore", "About SpeakMore"],
    ));
    let services = action!(services, "服务", "Services");
    let hide = action!(hide, "隐藏 SpeakMore", "Hide SpeakMore");
    let hide_others = action!(hide_others, "隐藏其他应用", "Hide Others");
    let show_all = action!(show_all, "显示全部", "Show All");
    let quit = action!(quit, "退出 SpeakMore", "Quit SpeakMore");
    let application = Submenu::with_items(
        app,
        "SpeakMore",
        true,
        &[
            &about,
            &PredefinedMenuItem::separator(app)?,
            &services,
            &PredefinedMenuItem::separator(app)?,
            &hide,
            &hide_others,
            &show_all,
            &PredefinedMenuItem::separator(app)?,
            &quit,
        ],
    )?;
    let close_file = action!(close_window, "关闭窗口", "Close Window");
    let file = submenu!("speakmore-file", "文件", "File", &[&close_file]);
    let undo = action!(undo, "撤销", "Undo");
    let redo = action!(redo, "重做", "Redo");
    let cut = action!(cut, "剪切", "Cut");
    let copy = action!(copy, "复制", "Copy");
    let paste = action!(paste, "粘贴", "Paste");
    let select_all = action!(select_all, "全选", "Select All");
    let edit = submenu!(
        "speakmore-edit",
        "编辑",
        "Edit",
        &[
            &undo,
            &redo,
            &PredefinedMenuItem::separator(app)?,
            &cut,
            &copy,
            &paste,
            &select_all,
        ]
    );
    let fullscreen = action!(fullscreen, "切换全屏", "Toggle Full Screen");
    let view = submenu!("speakmore-view", "显示", "View", &[&fullscreen]);
    let minimize = action!(minimize, "最小化", "Minimize");
    let zoom = action!(maximize, "缩放", "Zoom");
    let close_window = action!(close_window, "关闭窗口", "Close Window");
    let window = submenu!(
        WINDOW_SUBMENU_ID,
        "窗口",
        "Window",
        &[
            &minimize,
            &zoom,
            &PredefinedMenuItem::separator(app)?,
            &close_window,
        ]
    );
    let help = submenu!(HELP_SUBMENU_ID, "帮助", "Help", &[]);
    let menu = Menu::with_items(app, &[&application, &file, &edit, &view, &window, &help])?;
    app.set_menu(menu)?;
    // Register roles explicitly: the saved application language can differ
    // from the macOS language used to recognize the default Help title.
    window.set_as_windows_menu_for_nsapp()?;
    help.set_as_help_menu_for_nsapp()?;
    Ok(Labels(labels))
}
