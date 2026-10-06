use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use eframe::egui::{
    self, Align, Color32, CornerRadius, FontId, Layout, Margin, RichText, Sense, Stroke, Vec2,
};

use crate::config;
use crate::git::{self, RepoStatus, Tracking};

/// Number of repositories checked concurrently.
const WORKERS: usize = 6;

const INTERVAL_CHOICES: &[(u64, &str)] = &[
    (0, "off"),
    (1, "1 min"),
    (2, "2 min"),
    (5, "5 min"),
    (10, "10 min"),
    (15, "15 min"),
    (30, "30 min"),
    (60, "1 hour"),
];

mod palette {
    use eframe::egui::Color32;

    pub const BG: Color32 = Color32::from_rgb(0x16, 0x18, 0x1d);
    pub const PANEL: Color32 = Color32::from_rgb(0x1e, 0x21, 0x28);
    pub const ROW_HOVER: Color32 = Color32::from_rgb(0x26, 0x2a, 0x33);
    pub const TEXT: Color32 = Color32::from_rgb(0xd6, 0xda, 0xe1);
    pub const DIM: Color32 = Color32::from_rgb(0x7d, 0x85, 0x90);

    pub const GREEN: Color32 = Color32::from_rgb(0x3f, 0xd0, 0x6a);
    pub const BLUE: Color32 = Color32::from_rgb(0x4c, 0x9a, 0xff);
    pub const VIOLET: Color32 = Color32::from_rgb(0xb0, 0x7c, 0xff);
    pub const RED: Color32 = Color32::from_rgb(0xf8, 0x51, 0x49);
    pub const AMBER: Color32 = Color32::from_rgb(0xe3, 0xa7, 0x2f);
    pub const GRAY: Color32 = Color32::from_rgb(0x5a, 0x61, 0x6b);
}

enum State {
    Unknown,
    Ok(RepoStatus),
    Error(String),
}

struct Repo {
    id: u64,
    /// Path as written in the list file.
    path: PathBuf,
    /// Path resolved against the list file's directory.
    resolved: PathBuf,
    name: String,
    state: State,
    /// A local-only check is queued or running.
    pending_check: bool,
    /// A fetch is queued or running.
    pending_fetch: bool,
    /// A fetch result has been received at least once.
    fetched: bool,
    checked_at: Option<Instant>,
}

struct Job {
    id: u64,
    path: PathBuf,
    fetch: bool,
}

struct Done {
    id: u64,
    fetch: bool,
    result: Result<RepoStatus, String>,
}

pub struct MeerkatApp {
    list_file: PathBuf,
    repos: Vec<Repo>,
    next_id: u64,

    jobs: Sender<Job>,
    results: Receiver<Done>,

    interval: Duration,
    next_auto: Option<Instant>,

    add_open: bool,
    add_text: String,
    always_on_top: bool,
    /// Transient message shown in the footer (errors, hints).
    notice: Option<(String, Instant)>,
    title: String,
}

impl MeerkatApp {
    pub fn new(cc: &eframe::CreationContext<'_>, list_file: PathBuf, interval: Duration) -> Self {
        setup_style(&cc.egui_ctx);

        let (job_tx, job_rx) = channel::<Job>();
        let (msg_tx, msg_rx) = channel::<Done>();
        let job_rx = Arc::new(Mutex::new(job_rx));
        for _ in 0..WORKERS {
            let job_rx = Arc::clone(&job_rx);
            let msg_tx = msg_tx.clone();
            let ctx = cc.egui_ctx.clone();
            std::thread::spawn(move || worker(job_rx, msg_tx, ctx));
        }

        let mut app = Self {
            list_file,
            repos: Vec::new(),
            next_id: 0,
            jobs: job_tx,
            results: msg_rx,
            interval,
            next_auto: None,
            add_open: false,
            add_text: String::new(),
            always_on_top: false,
            notice: None,
            title: String::new(),
        };
        app.load_list();
        if app.repos.is_empty() {
            app.add_open = true;
        }
        // Show local state immediately, then fetch everything.
        app.check_all(false);
        app.fetch_all();
        app
    }

    // ---------------------------------------------------------------------
    // Repository list

    fn base_dir(&self) -> PathBuf {
        self.list_file
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_default()
    }

    fn make_repo(&mut self, path: PathBuf) -> Repo {
        let resolved = if path.is_absolute() {
            path.clone()
        } else {
            self.base_dir().join(&path)
        };
        let name = resolved
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| resolved.to_string_lossy().into_owned());
        self.next_id += 1;
        Repo {
            id: self.next_id,
            path,
            resolved,
            name,
            state: State::Unknown,
            pending_check: false,
            pending_fetch: false,
            fetched: false,
            checked_at: None,
        }
    }

    fn load_list(&mut self) {
        match config::load(&self.list_file) {
            Ok(paths) => {
                let mut old = std::mem::take(&mut self.repos);
                for path in paths {
                    // Keep state of repositories that were already listed.
                    if let Some(i) = old.iter().position(|r| r.path == path) {
                        self.repos.push(old.remove(i));
                    } else {
                        let repo = self.make_repo(path);
                        self.repos.push(repo);
                    }
                }
            }
            Err(e) => self.notify(format!("cannot read {}: {e}", self.list_file.display())),
        }
    }

    fn save_list(&mut self) {
        let paths: Vec<PathBuf> = self.repos.iter().map(|r| r.path.clone()).collect();
        if let Err(e) = config::save(&self.list_file, &paths) {
            self.notify(format!("cannot save {}: {e}", self.list_file.display()));
        }
    }

    fn add_repo(&mut self, input: &str) {
        let input = input.trim().trim_matches('"');
        if input.is_empty() {
            return;
        }
        let mut path = PathBuf::from(input);
        if !path.is_absolute() {
            path = std::path::absolute(&path).unwrap_or(path);
        }
        if self
            .repos
            .iter()
            .any(|r| r.path == path || r.resolved == path)
        {
            self.notify(format!("already listed: {}", path.display()));
            return;
        }
        if !path.is_dir() {
            self.notify(format!("not a directory: {}", path.display()));
            return;
        }
        let repo = self.make_repo(path);
        let id = repo.id;
        self.repos.push(repo);
        self.save_list();
        self.enqueue(id, true);
    }

    fn remove_repo(&mut self, id: u64) {
        self.repos.retain(|r| r.id != id);
        self.save_list();
    }

    fn move_repo(&mut self, id: u64, delta: isize) {
        let Some(i) = self.repos.iter().position(|r| r.id == id) else {
            return;
        };
        let j = i as isize + delta;
        if j >= 0 && (j as usize) < self.repos.len() {
            self.repos.swap(i, j as usize);
            self.save_list();
        }
    }

    // ---------------------------------------------------------------------
    // Background checks

    fn enqueue(&mut self, id: u64, fetch: bool) {
        let Some(repo) = self.repos.iter_mut().find(|r| r.id == id) else {
            return;
        };
        // Don't pile up jobs for a repository that is already being checked.
        let pending = if fetch {
            &mut repo.pending_fetch
        } else {
            &mut repo.pending_check
        };
        if *pending {
            return;
        }
        *pending = true;
        let _ = self.jobs.send(Job {
            id,
            path: repo.resolved.clone(),
            fetch,
        });
    }

    fn check_all(&mut self, fetch: bool) {
        let ids: Vec<u64> = self.repos.iter().map(|r| r.id).collect();
        for id in ids {
            self.enqueue(id, fetch);
        }
    }

    fn fetch_all(&mut self) {
        self.check_all(true);
        self.schedule_next();
    }

    fn schedule_next(&mut self) {
        self.next_auto = (!self.interval.is_zero()).then(|| Instant::now() + self.interval);
    }

    fn poll_results(&mut self) {
        while let Ok(Done { id, fetch, result }) = self.results.try_recv() {
            let Some(repo) = self.repos.iter_mut().find(|r| r.id == id) else {
                continue;
            };
            if fetch {
                repo.pending_fetch = false;
                repo.fetched = true;
            } else {
                repo.pending_check = false;
                // A quick local check must not overwrite a fresher fetch result.
                if repo.fetched {
                    continue;
                }
            }
            repo.state = match result {
                Ok(status) => State::Ok(status),
                Err(e) => State::Error(e),
            };
            repo.checked_at = Some(Instant::now());
        }
    }

    fn busy(&self) -> bool {
        self.repos.iter().any(|r| r.pending_fetch)
    }

    fn notify(&mut self, text: String) {
        self.notice = Some((text, Instant::now()));
    }

    // ---------------------------------------------------------------------
    // UI

    fn header(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            let busy = self.busy();
            let reload = ui
                .add_enabled(!busy, icon_button("⟳"))
                .on_hover_text("Fetch all now (F5)");
            if reload.clicked() {
                self.fetch_all();
            }
            if busy {
                ui.add(egui::Spinner::new().size(12.0));
            }
            ui.label(self.summary_text());

            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                self.menu(ui);
                let add = ui
                    .add(icon_button(if self.add_open { "−" } else { "+" }))
                    .on_hover_text("Add repository");
                if add.clicked() {
                    self.add_open = !self.add_open;
                }
                if let Some(next) = self.next_auto {
                    let left = next.saturating_duration_since(Instant::now());
                    ui.label(
                        RichText::new(format_short(left))
                            .color(palette::DIM)
                            .small(),
                    )
                    .on_hover_text("Time to next automatic fetch");
                }
            });
        });

        if self.add_open {
            ui.add_space(2.0);
            ui.horizontal(|ui| {
                let edit = ui.add(
                    egui::TextEdit::singleline(&mut self.add_text)
                        .hint_text("path, or drop folders here")
                        .desired_width(ui.available_width() - 34.0),
                );
                let enter = edit.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                if ui.add(icon_button("✔")).on_hover_text("Add").clicked() || enter {
                    let text = std::mem::take(&mut self.add_text);
                    self.add_repo(&text);
                    edit.request_focus();
                }
            });
        }
    }

    fn menu(&mut self, ui: &mut egui::Ui) {
        let button = egui::containers::menu::MenuButton::from_button(icon_button("☰"));
        button.ui(ui, |ui| {
            ui.label(RichText::new("Auto fetch").color(palette::DIM).small());
            let current = self.interval.as_secs() / 60;
            for &(minutes, label) in INTERVAL_CHOICES {
                if ui
                    .radio(
                        current == minutes && (minutes > 0 || self.interval.is_zero()),
                        label,
                    )
                    .clicked()
                {
                    self.interval = Duration::from_secs(minutes * 60);
                    self.schedule_next();
                }
            }
            ui.separator();
            if ui
                .checkbox(&mut self.always_on_top, "Always on top")
                .changed()
            {
                let level = if self.always_on_top {
                    egui::WindowLevel::AlwaysOnTop
                } else {
                    egui::WindowLevel::Normal
                };
                ui.ctx()
                    .send_viewport_cmd(egui::ViewportCommand::WindowLevel(level));
            }
            if ui.button("Reload list file").clicked() {
                self.load_list();
                self.check_all(false);
                self.fetch_all();
                ui.close();
            }
            if ui.button("Open list file").clicked() {
                if !self.list_file.exists() {
                    self.save_list();
                }
                open_path(&self.list_file);
                ui.close();
            }
            ui.separator();
            ui.label(
                RichText::new(self.list_file.display().to_string())
                    .color(palette::DIM)
                    .small(),
            );
        });
    }

    fn summary_text(&self) -> RichText {
        let (mut ok, mut attention) = (0, 0);
        for r in &self.repos {
            match &r.state {
                State::Ok(s) if s.sync == Tracking::InSync => ok += 1,
                State::Unknown => {}
                _ => attention += 1,
            }
        }
        let text = if self.repos.is_empty() {
            "no repositories".to_owned()
        } else if attention == 0 {
            format!("{ok}/{} in sync", self.repos.len())
        } else {
            format!("{ok}/{} in sync · {attention} to check", self.repos.len())
        };
        RichText::new(text).color(palette::DIM).small()
    }

    fn list(&mut self, ui: &mut egui::Ui) {
        let mut actions: Vec<(u64, Action)> = Vec::new();
        let row_h = ui.text_style_height(&egui::TextStyle::Body) + 6.0;

        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.spacing_mut().item_spacing.y = 1.0;
                for repo in &self.repos {
                    if let Some(action) = repo_row(ui, repo, row_h) {
                        actions.push((repo.id, action));
                    }
                }
                if self.repos.is_empty() {
                    ui.add_space(8.0);
                    ui.vertical_centered(|ui| {
                        ui.label(
                            RichText::new("Add a repository with + or drop a folder here")
                                .color(palette::DIM),
                        );
                    });
                }
            });

        for (id, action) in actions {
            match action {
                Action::Fetch => self.enqueue(id, true),
                Action::Remove => self.remove_repo(id),
                Action::Move(d) => self.move_repo(id, d),
            }
        }
    }

    fn footer(&mut self, ui: &mut egui::Ui) {
        if let Some((text, _)) = &self.notice {
            ui.horizontal(|ui| {
                ui.label(RichText::new(text).color(palette::AMBER).small());
            });
        }
    }

    fn handle_drops(&mut self, ctx: &egui::Context) {
        let dropped: Vec<PathBuf> = ctx.input(|i| {
            i.raw
                .dropped_files
                .iter()
                .map(|f| f.path().to_path_buf())
                .filter(|p| !p.as_os_str().is_empty())
                .collect()
        });
        for path in dropped {
            self.add_repo(&path.to_string_lossy());
        }
    }

    fn update_title(&mut self, ctx: &egui::Context) {
        let (mut ahead, mut behind, mut diverged) = (0, 0, 0);
        for r in &self.repos {
            if let State::Ok(s) = &r.state {
                match s.sync {
                    Tracking::Ahead(_) => ahead += 1,
                    Tracking::Behind(_) => behind += 1,
                    Tracking::Diverged { .. } => diverged += 1,
                    _ => {}
                }
            }
        }
        let mut title = String::from("meerkat");
        let parts: Vec<String> = [(ahead, "ahead"), (behind, "behind"), (diverged, "diverged")]
            .iter()
            .filter(|(n, _)| *n > 0)
            .map(|(n, l)| format!("{n} {l}"))
            .collect();
        if !parts.is_empty() {
            title = format!("meerkat — {}", parts.join(", "));
        }
        if title != self.title {
            ctx.send_viewport_cmd(egui::ViewportCommand::Title(title.clone()));
            self.title = title;
        }
    }
}

impl eframe::App for MeerkatApp {
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.poll_results();

        if let Some(next) = self.next_auto
            && Instant::now() >= next
        {
            self.fetch_all();
        }
        if ctx.input(|i| i.key_pressed(egui::Key::F5)) && !self.busy() {
            self.fetch_all();
        }
        if let Some((_, at)) = &self.notice
            && at.elapsed() > Duration::from_secs(6)
        {
            self.notice = None;
        }

        self.handle_drops(ctx);
        self.update_title(ctx);

        // Wake up for the next auto fetch, and periodically to refresh the
        // countdown; workers request a repaint themselves when they finish.
        let mut wake = Duration::from_secs(if self.notice.is_some() { 1 } else { 15 });
        if let Some(next) = self.next_auto {
            let left = next.saturating_duration_since(Instant::now());
            if left < Duration::from_secs(90) {
                wake = wake.min(Duration::from_secs(1));
            }
            wake = wake.min(left);
        }
        ctx.request_repaint_after(wake);
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();

        egui::Panel::top("header")
            .frame(
                egui::Frame::new()
                    .fill(palette::PANEL)
                    .inner_margin(Margin::symmetric(6, 4)),
            )
            .show(ui, |ui| self.header(ui));

        if self.notice.is_some() {
            egui::Panel::bottom("footer")
                .frame(
                    egui::Frame::new()
                        .fill(palette::PANEL)
                        .inner_margin(Margin::symmetric(6, 2)),
                )
                .show(ui, |ui| self.footer(ui));
        }

        egui::CentralPanel::default()
            .frame(
                egui::Frame::new()
                    .fill(palette::BG)
                    .inner_margin(Margin::symmetric(2, 3)),
            )
            .show(ui, |ui| self.list(ui));

        // Highlight the window while folders are dragged over it.
        if ctx.input(|i| !i.raw.hovered_files.is_empty()) {
            let rect = ctx.content_rect();
            ctx.layer_painter(egui::LayerId::new(
                egui::Order::Foreground,
                egui::Id::new("drop"),
            ))
            .rect_stroke(
                rect.shrink(2.0),
                CornerRadius::same(4),
                Stroke::new(2.0, palette::BLUE),
                egui::StrokeKind::Inside,
            );
        }
    }

    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        palette::BG.to_normalized_gamma_f32()
    }
}

enum Action {
    Fetch,
    Remove,
    Move(isize),
}

/// One compact row: status light, name, branch, ahead/behind counters.
fn repo_row(ui: &mut egui::Ui, repo: &Repo, row_h: f32) -> Option<Action> {
    let (rect, response) =
        ui.allocate_exact_size(Vec2::new(ui.available_width(), row_h), Sense::click());
    let painter = ui.painter_at(rect);

    if response.hovered() || response.context_menu_opened() {
        painter.rect_filled(rect, CornerRadius::same(3), palette::ROW_HOVER);
    }

    let (color, counters) = describe(&repo.state);
    let body = FontId::proportional(ui.text_style_height(&egui::TextStyle::Body) - 1.0);
    let small = FontId::proportional(body.size - 1.5);
    let cy = rect.center().y;

    // Status light, drawn as vector shapes so it stays crisp at any DPI.
    let center = egui::pos2(rect.left() + 11.0, cy);
    let r = 4.5;
    if matches!(repo.state, State::Unknown) {
        painter.circle_stroke(center, r - 0.5, Stroke::new(1.2, color));
    } else {
        painter.circle_filled(center, r + 2.5, color.gamma_multiply(0.18));
        painter.circle_filled(center, r, color);
    }

    let mut x = rect.left() + 22.0;

    // Right side, laid out from the right edge inward.
    let mut right = rect.right() - 6.0;
    if repo.pending_fetch {
        let size = 10.0;
        let spin =
            egui::Rect::from_center_size(egui::pos2(right - size / 2.0, cy), Vec2::splat(size));
        ui.put(spin, egui::Spinner::new().size(size).color(palette::DIM));
        right -= size + 6.0;
    }
    for (text, c) in counters.iter().rev() {
        let galley = painter.layout_no_wrap(text.clone(), body.clone(), *c);
        right -= galley.size().x;
        painter.galley(egui::pos2(right, cy - galley.size().y / 2.0), galley, *c);
        right -= 5.0;
    }
    if let State::Ok(s) = &repo.state
        && s.fetch_error.is_some()
    {
        let galley = painter.layout_no_wrap("⚠".into(), small.clone(), palette::AMBER);
        right -= galley.size().x;
        painter.galley(
            egui::pos2(right, cy - galley.size().y / 2.0),
            galley,
            palette::AMBER,
        );
        right -= 5.0;
    }

    // Left side: name, dirty marker, branch (elided to the available space).
    let avail = (right - x - 4.0).max(0.0);
    let name = elided(ui, &repo.name, body.clone(), palette::TEXT, avail);
    let name_w = name.size().x;
    painter.galley(egui::pos2(x, cy - name.size().y / 2.0), name, palette::TEXT);
    x += name_w;

    if let State::Ok(s) = &repo.state {
        if s.dirty {
            let g = painter.layout_no_wrap("*".into(), body.clone(), palette::AMBER);
            let w = g.size().x;
            painter.galley(
                egui::pos2(x + 1.0, cy - g.size().y / 2.0),
                g,
                palette::AMBER,
            );
            x += w + 1.0;
        }
        let avail = right - x - 12.0;
        if avail > 20.0 {
            let g = elided(ui, &s.branch, small.clone(), palette::DIM, avail);
            painter.galley(egui::pos2(x + 8.0, cy - g.size().y / 2.0), g, palette::DIM);
        }
    }

    let response = response.on_hover_ui(|ui| tooltip(ui, repo));

    let mut action = None;
    if response.double_clicked() {
        open_path(&repo.resolved);
    }
    response.context_menu(|ui| {
        if ui.button("⟳  Fetch now").clicked() {
            action = Some(Action::Fetch);
            ui.close();
        }
        if ui.button("🗁  Open folder").clicked() {
            open_path(&repo.resolved);
            ui.close();
        }
        if ui.button("Copy path").clicked() {
            ui.ctx()
                .copy_text(repo.resolved.to_string_lossy().into_owned());
            ui.close();
        }
        ui.separator();
        if ui.button("⏶  Move up").clicked() {
            action = Some(Action::Move(-1));
            ui.close();
        }
        if ui.button("⏷  Move down").clicked() {
            action = Some(Action::Move(1));
            ui.close();
        }
        ui.separator();
        if ui.button("✖  Remove").clicked() {
            action = Some(Action::Remove);
            ui.close();
        }
    });
    action
}

fn tooltip(ui: &mut egui::Ui, repo: &Repo) {
    ui.label(RichText::new(&repo.name).strong());
    ui.label(
        RichText::new(repo.resolved.display().to_string())
            .color(palette::DIM)
            .small(),
    );
    ui.add_space(2.0);
    match &repo.state {
        State::Unknown => {
            ui.label("not checked yet");
        }
        State::Error(e) => {
            ui.label(RichText::new(e).color(palette::AMBER));
        }
        State::Ok(s) => {
            let target = s.upstream.as_deref().unwrap_or("—");
            ui.label(format!("{} → {}", s.branch, target));
            let text = match s.sync {
                Tracking::InSync => "in sync".to_owned(),
                Tracking::Ahead(n) => format!("{n} commit(s) to push"),
                Tracking::Behind(n) => format!("{n} commit(s) to pull"),
                Tracking::Diverged { ahead, behind } => {
                    format!("diverged: {ahead} to push, {behind} to pull")
                }
                Tracking::NoUpstream => "no upstream branch".to_owned(),
                Tracking::Detached => "detached HEAD".to_owned(),
            };
            ui.label(text);
            if s.dirty {
                ui.label(RichText::new("uncommitted changes").color(palette::AMBER));
            }
            if let Some(e) = &s.fetch_error {
                ui.label(RichText::new(format!("fetch failed: {e}")).color(palette::AMBER));
            }
        }
    }
    if let Some(at) = repo.checked_at {
        ui.label(
            RichText::new(format!("checked {} ago", format_short(at.elapsed())))
                .color(palette::DIM)
                .small(),
        );
    }
}

/// Status light colour and the right-hand counters for a repository.
fn describe(state: &State) -> (Color32, Vec<(String, Color32)>) {
    match state {
        State::Unknown => (palette::GRAY, vec![]),
        State::Error(_) => (palette::AMBER, vec![("error".into(), palette::AMBER)]),
        State::Ok(s) => match s.sync {
            Tracking::InSync => (palette::GREEN, vec![]),
            Tracking::Ahead(n) => (palette::BLUE, vec![(format!("↑{n}"), palette::BLUE)]),
            Tracking::Behind(n) => (palette::VIOLET, vec![(format!("↓{n}"), palette::VIOLET)]),
            Tracking::Diverged { ahead, behind } => (
                palette::RED,
                vec![
                    (format!("↑{ahead}"), palette::BLUE),
                    (format!("↓{behind}"), palette::VIOLET),
                ],
            ),
            Tracking::NoUpstream => (palette::GRAY, vec![("no upstream".into(), palette::DIM)]),
            Tracking::Detached => (palette::GRAY, vec![]),
        },
    }
}

fn elided(
    ui: &egui::Ui,
    text: &str,
    font: FontId,
    color: Color32,
    max_width: f32,
) -> Arc<egui::Galley> {
    let mut job = egui::text::LayoutJob::simple_singleline(text.to_owned(), font, color);
    job.wrap = egui::text::TextWrapping::truncate_at_width(max_width);
    ui.fonts_mut(|f| f.layout_job(job))
}

fn icon_button(text: &str) -> egui::Button<'_> {
    egui::Button::new(RichText::new(text).size(14.0))
        .frame(false)
        .min_size(Vec2::splat(20.0))
}

fn format_short(d: Duration) -> String {
    let s = d.as_secs();
    if s < 60 {
        format!("{s}s")
    } else if s < 3600 {
        format!("{}m", s / 60)
    } else {
        format!("{}h{:02}m", s / 3600, (s % 3600) / 60)
    }
}

fn worker(jobs: Arc<Mutex<Receiver<Job>>>, out: Sender<Done>, ctx: egui::Context) {
    loop {
        let job = {
            let Ok(rx) = jobs.lock() else { return };
            match rx.recv() {
                Ok(job) => job,
                Err(_) => return,
            }
        };
        let result = git::check(&job.path, job.fetch);
        if out
            .send(Done {
                id: job.id,
                fetch: job.fetch,
                result,
            })
            .is_err()
        {
            return;
        }
        ctx.request_repaint();
    }
}

/// Open a file or folder with the platform's default handler.
fn open_path(path: &Path) {
    #[cfg(windows)]
    let mut cmd = {
        let mut c = std::process::Command::new("explorer");
        c.arg(path);
        c
    };
    #[cfg(target_os = "macos")]
    let mut cmd = {
        let mut c = std::process::Command::new("open");
        c.arg(path);
        c
    };
    #[cfg(all(unix, not(target_os = "macos")))]
    let mut cmd = {
        let mut c = std::process::Command::new("xdg-open");
        c.arg(path);
        c
    };
    let _ = cmd
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn();
}

fn setup_style(ctx: &egui::Context) {
    // The proportional font lacks arrows (↑ ↓ →); fall back to the bundled
    // monospace font for those glyphs.
    let mut fonts = egui::FontDefinitions::default();
    if let Some(family) = fonts.families.get_mut(&egui::FontFamily::Proportional) {
        family.push("Hack".to_owned());
    }
    ctx.set_fonts(fonts);

    ctx.set_theme(egui::ThemePreference::Dark);
    ctx.style_mut_of(egui::Theme::Dark, |style| {
        let v = &mut style.visuals;
        v.panel_fill = palette::BG;
        v.window_fill = palette::PANEL;
        v.extreme_bg_color = Color32::from_rgb(0x10, 0x12, 0x16);
        v.override_text_color = Some(palette::TEXT);
        v.window_corner_radius = CornerRadius::same(6);
        v.menu_corner_radius = CornerRadius::same(6);
        v.selection.bg_fill = palette::BLUE.gamma_multiply(0.5);
        v.widgets.hovered.weak_bg_fill = palette::ROW_HOVER;
        v.widgets.active.weak_bg_fill = palette::ROW_HOVER;

        let s = &mut style.spacing;
        s.item_spacing = Vec2::new(6.0, 3.0);
        s.button_padding = Vec2::new(6.0, 2.0);
        s.interact_size.y = 18.0;
        s.menu_margin = Margin::same(6);
    });
}

/// A small window icon: a status light on a dark rounded square.
pub fn icon() -> egui::IconData {
    const N: usize = 64;
    let mut rgba = vec![0u8; N * N * 4];
    let c = (N as f32 - 1.0) / 2.0;
    for y in 0..N {
        for x in 0..N {
            let (dx, dy) = (x as f32 - c, y as f32 - c);
            let i = (y * N + x) * 4;
            // Rounded square background.
            let q = (dx.abs() - 22.0).max(0.0).hypot((dy.abs() - 22.0).max(0.0));
            let bg_a = (10.0 - q).clamp(0.0, 1.0);
            let mut px = [0x16 as f32, 0x18 as f32, 0x1d as f32, 255.0 * bg_a];
            // Green light with a soft halo.
            let d = dx.hypot(dy);
            let halo = ((26.0 - d) / 10.0).clamp(0.0, 1.0) * 0.35;
            let dot = (17.0 - d).clamp(0.0, 1.0);
            let a = dot.max(halo);
            let g = palette::GREEN;
            for (k, ch) in [g.r(), g.g(), g.b()].into_iter().enumerate() {
                px[k] = px[k] * (1.0 - a) + ch as f32 * a;
            }
            for k in 0..4 {
                rgba[i + k] = px[k].round() as u8;
            }
        }
    }
    egui::IconData {
        rgba,
        width: N as u32,
        height: N as u32,
    }
}
