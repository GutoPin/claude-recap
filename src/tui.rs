//! The interactive view: a list on the left, the recap of the selected item
//! on the right.

use std::fmt::Display;
use std::io;

use chrono::{DateTime, Local, TimeZone, Utc};
use ratatui::Frame;
use ratatui::crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, List, ListItem, ListState, Paragraph};

use crate::recap::{Row, Tone, ago, day, folder_rows, money, session_rows, skill_rows, tilde};
use crate::store::{Folder, Library};

const ACCENT: Color = Color::Indexed(173);
const GREY: Color = Color::Indexed(244);
const WIDE: u16 = 90;
const PAGE: u16 = 10;

#[derive(Debug, Clone, Copy, PartialEq)]
enum View {
    Folders,
    Sessions,
    Skills,
}

pub struct App<'a> {
    lib: &'a Library,
    now: DateTime<Utc>,
    view: View,
    /// Where `esc` returns to from the skills list.
    before_skills: View,
    /// Index into `lib.folders` of the folder whose sessions are open.
    open: usize,
    /// Position in the filtered list of each view.
    folder: usize,
    session: usize,
    skill: usize,
    filter: String,
    typing: bool,
    scroll: u16,
    list: ListState,
}

pub fn run(lib: &Library) -> io::Result<()> {
    let mut app = App::new(lib, Utc::now());
    let mut terminal = ratatui::init();
    let result = (|| {
        loop {
            terminal.draw(|f| app.draw(f, &Local))?;
            if let Event::Key(key) = event::read()?
                && key.kind == KeyEventKind::Press
                && app.on_key(key)
            {
                return Ok(());
            }
        }
    })();
    ratatui::restore();
    result
}

impl<'a> App<'a> {
    pub fn new(lib: &'a Library, now: DateTime<Utc>) -> Self {
        App {
            lib,
            now,
            view: View::Folders,
            before_skills: View::Folders,
            open: 0,
            folder: 0,
            session: 0,
            skill: 0,
            filter: String::new(),
            typing: false,
            scroll: 0,
            list: ListState::default(),
        }
    }

    fn matches(&self, haystack: &[&str]) -> bool {
        let needle = self.filter.trim().to_lowercase();
        needle.is_empty() || haystack.iter().any(|h| h.to_lowercase().contains(&needle))
    }

    fn folders(&self) -> Vec<usize> {
        (0..self.lib.folders.len())
            .filter(|&i| self.matches(&[&self.lib.folders[i].path]))
            .collect()
    }

    fn open_folder(&self) -> Option<&'a Folder> {
        self.lib.folders.get(self.open)
    }

    fn sessions(&self) -> Vec<usize> {
        let Some(folder) = self.open_folder() else {
            return Vec::new();
        };
        (0..folder.sessions.len())
            .filter(|&i| {
                let s = &folder.sessions[i];
                let name = s.name();
                let mut fields = vec![name.as_str()];
                fields.extend(s.prompts.iter().map(|p| p.text.as_str()));
                self.matches(&fields)
            })
            .collect()
    }

    fn skills(&self) -> Vec<usize> {
        (0..self.lib.skills.len())
            .filter(|&i| {
                let s = &self.lib.skills[i];
                self.matches(&[&s.name, &s.description, &s.source.label()])
            })
            .collect()
    }

    fn len(&self) -> usize {
        match self.view {
            View::Folders => self.folders().len(),
            View::Sessions => self.sessions().len(),
            View::Skills => self.skills().len(),
        }
    }

    fn cursor(&mut self) -> &mut usize {
        match self.view {
            View::Folders => &mut self.folder,
            View::Sessions => &mut self.session,
            View::Skills => &mut self.skill,
        }
    }

    fn go(&mut self, to: usize) {
        let last = self.len().saturating_sub(1);
        *self.cursor() = to.min(last);
        self.scroll = 0;
    }

    fn step(&mut self, by: isize) {
        let at = *self.cursor();
        self.go(at.saturating_add_signed(by));
    }

    fn show(&mut self, view: View) {
        self.view = view;
        self.filter.clear();
        self.typing = false;
        self.scroll = 0;
        self.list = ListState::default();
    }

    fn back(&mut self) {
        match self.view {
            View::Folders => {}
            View::Sessions => self.show(View::Folders),
            View::Skills => self.show(self.before_skills),
        }
    }

    /// Returns true when the app should quit.
    pub fn on_key(&mut self, key: KeyEvent) -> bool {
        if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
            return true;
        }
        if self.typing {
            match key.code {
                KeyCode::Enter => self.typing = false,
                KeyCode::Esc => {
                    self.typing = false;
                    self.filter.clear();
                }
                KeyCode::Backspace => {
                    self.filter.pop();
                }
                KeyCode::Char(c) => self.filter.push(c),
                _ => {}
            }
            self.go(0);
            return false;
        }
        match key.code {
            KeyCode::Char('q') => return true,
            KeyCode::Up | KeyCode::Char('k') => self.step(-1),
            KeyCode::Down | KeyCode::Char('j') => self.step(1),
            KeyCode::Home | KeyCode::Char('g') => self.go(0),
            KeyCode::End | KeyCode::Char('G') => self.go(usize::MAX),
            KeyCode::PageDown | KeyCode::Char(' ') => {
                self.scroll = self.scroll.saturating_add(PAGE)
            }
            KeyCode::PageUp | KeyCode::Char('b') => self.scroll = self.scroll.saturating_sub(PAGE),
            KeyCode::Char('/') => self.typing = true,
            KeyCode::Enter | KeyCode::Right | KeyCode::Char('l') if self.view == View::Folders => {
                if let Some(&folder) = self.folders().get(self.folder) {
                    self.open = folder;
                    self.session = 0;
                    self.show(View::Sessions);
                }
            }
            KeyCode::Esc if !self.filter.is_empty() => {
                self.filter.clear();
                self.go(0);
            }
            KeyCode::Esc | KeyCode::Left | KeyCode::Char('h') | KeyCode::Backspace => self.back(),
            KeyCode::Char('s') if self.view == View::Skills => self.back(),
            KeyCode::Char('s') => {
                self.before_skills = self.view;
                self.show(View::Skills);
            }
            _ => {}
        }
        false
    }

    pub fn draw<Tz: TimeZone>(&mut self, f: &mut Frame, tz: &Tz)
    where
        Tz::Offset: Display,
    {
        let [header, body, footer] = Layout::vertical([
            Constraint::Length(1),
            Constraint::Min(0),
            Constraint::Length(1),
        ])
        .areas(f.area());
        let [left, right] = if body.width >= WIDE {
            Layout::horizontal([Constraint::Percentage(38), Constraint::Percentage(62)]).areas(body)
        } else {
            Layout::vertical([Constraint::Percentage(40), Constraint::Percentage(60)]).areas(body)
        };

        f.render_widget(Paragraph::new(self.breadcrumb()), header);
        let (items, rows) = self.content(left.width.saturating_sub(4).into(), tz);
        self.draw_list(f, left, items);
        self.draw_recap(f, right, rows);
        f.render_widget(
            Paragraph::new(self.hint()).style(Style::new().fg(GREY)),
            footer,
        );
    }

    fn breadcrumb(&self) -> Line<'static> {
        let mut parts = vec![Span::styled(
            " claude-recap",
            Style::new().fg(ACCENT).add_modifier(Modifier::BOLD),
        )];
        let mut crumb = |text: String| {
            parts.push(Span::styled("  ›  ", Style::new().fg(GREY)));
            parts.push(Span::raw(text));
        };
        match self.view {
            View::Folders => crumb(format!("Folders ({})", self.folders().len())),
            View::Sessions => {
                if let Some(folder) = self.open_folder() {
                    crumb(tilde(&folder.path));
                }
                crumb(format!("Sessions ({})", self.sessions().len()));
            }
            View::Skills => crumb(format!("Skills ({})", self.skills().len())),
        }
        Line::from(parts)
    }

    fn hint(&self) -> String {
        if self.typing {
            return format!(" filter: {}_   enter keep   esc clear", self.filter);
        }
        let filter = if self.filter.is_empty() {
            String::new()
        } else {
            format!("filter: {}   ", self.filter)
        };
        let keys = match self.view {
            View::Folders => {
                "↑↓ move   enter open   / filter   s skills   pgup/pgdn scroll   q quit"
            }
            View::Sessions => {
                "↑↓ move   esc back   / filter   s skills   pgup/pgdn scroll   q quit"
            }
            View::Skills => "↑↓ move   esc back   / filter   pgup/pgdn scroll   q quit",
        };
        format!(" {filter}{keys}")
    }

    /// The list entries of the current view and the recap of the selected one.
    fn content<Tz: TimeZone>(&self, width: usize, tz: &Tz) -> (Vec<String>, Vec<Row>)
    where
        Tz::Offset: Display,
    {
        match self.view {
            View::Folders => {
                let shown = self.folders();
                let items = shown
                    .iter()
                    .map(|&i| {
                        let folder = &self.lib.folders[i];
                        let when = folder
                            .last_active()
                            .map(|at| ago(self.now, at))
                            .unwrap_or_default();
                        let cost = folder.cost_usd().map(money).unwrap_or_default();
                        columns(
                            folder.name(),
                            &format!("{:>3}  {when:>8}  {cost:>8}", folder.sessions.len()),
                            width,
                        )
                    })
                    .collect();
                let rows = shown
                    .get(self.folder)
                    .map(|&i| folder_rows(&self.lib.folders[i], self.now, tz))
                    .unwrap_or_default();
                (items, rows)
            }
            View::Sessions => {
                let Some(folder) = self.open_folder() else {
                    return (Vec::new(), Vec::new());
                };
                let shown = self.sessions();
                let items = shown
                    .iter()
                    .map(|&i| {
                        let session = &folder.sessions[i];
                        let when = session
                            .ended
                            .map(|at| day(at, self.now, tz))
                            .unwrap_or_default();
                        columns(&session.name(), &when, width)
                    })
                    .collect();
                let rows = shown
                    .get(self.session)
                    .map(|&i| session_rows(&folder.sessions[i], self.now, tz))
                    .unwrap_or_default();
                (items, rows)
            }
            View::Skills => {
                let shown = self.skills();
                let items = shown
                    .iter()
                    .map(|&i| {
                        let skill = &self.lib.skills[i];
                        let uses = if skill.uses == 0 {
                            String::new()
                        } else {
                            format!("{}x", skill.uses)
                        };
                        columns(&skill.name, &uses, width)
                    })
                    .collect();
                let rows = shown
                    .get(self.skill)
                    .map(|&i| skill_rows(&self.lib.skills[i], self.now))
                    .unwrap_or_default();
                (items, rows)
            }
        }
    }

    fn draw_list(&mut self, f: &mut Frame, area: Rect, items: Vec<String>) {
        let title = match self.view {
            View::Folders => " Folders ",
            View::Sessions => " Sessions ",
            View::Skills => " Skills ",
        };
        let selected = *self.cursor();
        self.list.select((!items.is_empty()).then_some(selected));
        let list = List::new(items.into_iter().map(ListItem::new))
            .block(
                Block::bordered()
                    .title(title)
                    .border_style(Style::new().fg(GREY)),
            )
            .highlight_style(Style::new().add_modifier(Modifier::REVERSED))
            .highlight_symbol("› ");
        f.render_stateful_widget(list, area, &mut self.list);
    }

    fn draw_recap(&mut self, f: &mut Frame, area: Rect, rows: Vec<Row>) {
        let width = usize::from(area.width.saturating_sub(2));
        let lines: Vec<Line> = if rows.is_empty() {
            let message = if self.filter.is_empty() {
                "Nothing here yet."
            } else {
                "Nothing matches the filter."
            };
            vec![Line::styled(message, Style::new().fg(GREY))]
        } else {
            rows.into_iter()
                .flat_map(|(tone, text)| {
                    let style = match tone {
                        Tone::Title => Style::new().fg(ACCENT).add_modifier(Modifier::BOLD),
                        Tone::Section => Style::new().add_modifier(Modifier::BOLD),
                        Tone::Dim => Style::new().fg(GREY),
                        Tone::Plain => Style::new(),
                    };
                    wrap(&text, width)
                        .into_iter()
                        .map(move |line| Line::styled(line, style))
                })
                .collect()
        };
        // Stop scrolling once the last line is on screen.
        let inner_height = area.height.saturating_sub(2);
        self.scroll = self
            .scroll
            .min((lines.len() as u16).saturating_sub(inner_height.max(1)));
        let recap = Paragraph::new(lines)
            .block(
                Block::bordered()
                    .title(" Recap ")
                    .border_style(Style::new().fg(GREY)),
            )
            .scroll((self.scroll, 0));
        f.render_widget(recap, area);
    }
}

/// Breaks `text` into lines of at most `width` characters, at spaces where it
/// can. Done here instead of by the widget so the scroll limit can count the
/// lines that will really be drawn.
fn wrap(text: &str, width: usize) -> Vec<String> {
    let width = width.max(1);
    // Leading spaces are indentation and stay on the first line.
    let body = text.trim_start_matches(' ');
    let indent = (text.len() - body.len()).min(width - 1);
    let mut lines = Vec::new();
    let mut line = " ".repeat(indent);
    let mut len = indent;
    let mut fresh = true;
    for word in body.split(' ') {
        let mut word: Vec<char> = word.chars().collect();
        if !fresh && len + 1 + word.len() > width {
            lines.push(std::mem::take(&mut line));
            len = 0;
            fresh = true;
        }
        if !fresh {
            line.push(' ');
            len += 1;
        }
        // A word longer than the pane, such as a path, is split mid-word.
        while len + word.len() > width {
            let rest = word.split_off(width - len);
            line.extend(word);
            lines.push(std::mem::take(&mut line));
            word = rest;
            len = 0;
        }
        len += word.len();
        line.extend(word);
        fresh = false;
    }
    lines.push(line);
    lines
}

/// `left` and `right` on one line of `width` cells, `left` cut to make room.
fn columns(left: &str, right: &str, width: usize) -> String {
    let right_len = right.chars().count();
    let room = width.saturating_sub(right_len + 1).max(1);
    let left_len = left.chars().count();
    let left = if left_len > room {
        let kept: String = left.chars().take(room.saturating_sub(1)).collect();
        format!("{kept}…")
    } else {
        format!("{left:<room$}")
    };
    format!("{left} {right}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::{load, tests::fixture};
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    fn now() -> DateTime<Utc> {
        "2026-03-05T09:00:10Z".parse().unwrap()
    }

    fn screen(app: &mut App, width: u16, height: u16) -> String {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        terminal.draw(|f| app.draw(f, &Utc)).unwrap();
        let buffer = terminal.backend().buffer();
        (0..height)
            .map(|y| {
                (0..width)
                    .map(|x| buffer[(x, y)].symbol())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn press(app: &mut App, keys: &str) {
        for c in keys.chars() {
            let code = match c {
                '\n' => KeyCode::Enter,
                '\x1b' => KeyCode::Esc,
                c => KeyCode::Char(c),
            };
            assert!(!app.on_key(KeyEvent::from(code)), "quit on {c:?}");
        }
    }

    fn assert_has(text: &str, parts: &[&str]) {
        for part in parts {
            assert!(text.contains(part), "missing {part:?} in:\n{text}");
        }
    }

    #[test]
    fn folders_then_sessions_then_back() {
        let fx = fixture();
        let lib = load(&fx.0).unwrap();
        let mut app = App::new(&lib, now());

        let text = screen(&mut app, 120, 30);
        assert_has(
            &text,
            &[
                "claude-recap",
                "Folders (2)",
                "› shop",
                "notes",
                "2d ago",
                "$4.00",
            ],
        );
        assert_has(
            &text,
            &[
                "/work/shop",
                "2 sessions · last active 2d ago",
                "Mar 3   why is the build failing?",
                "Most edited files",
            ],
        );

        press(&mut app, "j");
        assert_has(
            &screen(&mut app, 120, 30),
            &[
                "› notes",
                "/work/notes",
                "1 session ·",
                "summarize my notes",
            ],
        );

        press(&mut app, "k\n");
        let text = screen(&mut app, 120, 30);
        assert_has(
            &text,
            &[
                "/work/shop  ›  Sessions (2)",
                "› why is the build failing?",
                "Cart and checkout",
                "claude --resume bbbb-2222",
            ],
        );

        press(&mut app, "j");
        assert_has(
            &screen(&mut app, 120, 30),
            &[
                "10:00  add a cart page",
                "10:20  now the checkout",
                "claude --resume aaaa-1111",
            ],
        );

        press(&mut app, "\x1b");
        assert_has(&screen(&mut app, 120, 30), &["Folders (2)", "› shop"]);
    }

    #[test]
    fn skills_are_one_key_away_from_anywhere() {
        let fx = fixture();
        let lib = load(&fx.0).unwrap();
        let mut app = App::new(&lib, now());

        press(&mut app, "\ns");
        let text = screen(&mut app, 120, 30);
        assert_has(
            &text,
            &[
                "Skills (5)",
                "› frontend-design",
                "1x",
                "tools:debugging",
                "unused",
            ],
        );
        assert_has(
            &text,
            &[
                "user · used 1 time · last used 3d ago",
                "Guidance for distinctive visual design.",
            ],
        );

        press(&mut app, "jj");
        assert_has(
            &screen(&mut app, 120, 30),
            &["plugin tools · used 1 time", "Find the root cause: first."],
        );

        press(&mut app, "s");
        assert_has(&screen(&mut app, 120, 30), &["Sessions (2)"]);
    }

    #[test]
    fn filter_narrows_the_current_list() {
        let fx = fixture();
        let lib = load(&fx.0).unwrap();
        let mut app = App::new(&lib, now());

        press(&mut app, "/NOTE");
        let text = screen(&mut app, 120, 30);
        assert_has(&text, &["Folders (1)", "› notes", "filter: NOTE_"]);
        assert!(!text.contains("shop"), "{text}");

        // While typing, letters are text, not commands.
        press(&mut app, "sq");
        assert_has(
            &screen(&mut app, 120, 30),
            &[
                "Folders (0)",
                "Nothing matches the filter.",
                "filter: NOTEsq_",
            ],
        );

        press(&mut app, "\x1b\n/checkout\n");
        let text = screen(&mut app, 120, 30);
        assert_has(
            &text,
            &["Sessions (1)", "› Cart and checkout", "filter: checkout   "],
        );
        press(&mut app, "\x1b");
        assert_has(&screen(&mut app, 120, 30), &["Sessions (2)"]);
    }

    #[test]
    fn narrow_and_tiny_terminals() {
        let fx = fixture();
        let lib = load(&fx.0).unwrap();
        let mut app = App::new(&lib, now());
        assert_has(
            &screen(&mut app, 60, 24),
            &["› shop", "/work/shop", "Sessions"],
        );
        for (w, h) in [(10, 3), (1, 1), (40, 8)] {
            screen(&mut app, w, h);
        }
    }

    #[test]
    fn recap_scrolls_and_stops_at_the_end() {
        let fx = fixture();
        let lib = load(&fx.0).unwrap();
        let mut app = App::new(&lib, now());
        press(&mut app, "\nj");
        let top = screen(&mut app, 120, 12);
        assert!(
            top.contains("Cart and checkout · ") || top.contains("Mar 1, 10:00"),
            "{top}"
        );

        press(&mut app, "   ");
        let bottom = screen(&mut app, 120, 12);
        assert_has(&bottom, &["frontend-design 1"]);
        assert!(!bottom.contains("Mar 1, 10:00"), "{bottom}");
        press(&mut app, "bbb");
        assert_has(&screen(&mut app, 120, 12), &["Mar 1, 10:00"]);
    }

    #[test]
    fn an_empty_library_draws() {
        let lib = Library {
            root: "/nowhere".into(),
            folders: Vec::new(),
            skills: Vec::new(),
        };
        let mut app = App::new(&lib, now());
        assert_has(
            &screen(&mut app, 100, 20),
            &["Folders (0)", "Nothing here yet."],
        );
        press(&mut app, "j\ns\x1bG");
        screen(&mut app, 100, 20);
    }

    #[test]
    fn q_quits_unless_typing() {
        let lib = Library {
            root: "/nowhere".into(),
            folders: Vec::new(),
            skills: Vec::new(),
        };
        let mut app = App::new(&lib, now());
        assert!(app.on_key(KeyEvent::from(KeyCode::Char('q'))));
        assert!(app.on_key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL)));
    }

    #[test]
    fn wraps_at_spaces_and_splits_long_words() {
        assert_eq!(wrap("one two three", 7), ["one two", "three"]);
        assert_eq!(wrap("   2  src/cart.ts", 40), ["   2  src/cart.ts"]);
        assert_eq!(
            wrap("see /a/very/long/path now", 8),
            ["see", "/a/very/", "long/pat", "h now"]
        );
        assert_eq!(
            wrap("10:00  add a cart page", 14),
            ["10:00  add a", "cart page"]
        );
        assert_eq!(wrap("", 10), [""]);
        assert_eq!(wrap("añadir línea", 6), ["añadir", "línea"]);
        assert!(
            wrap("anything at all", 0)
                .iter()
                .all(|l| l.chars().count() <= 1)
        );
    }

    #[test]
    fn columns_fit_the_width() {
        assert_eq!(columns("shop", "2d ago", 14), "shop    2d ago");
        assert_eq!(
            columns("a-very-long-folder", "2d ago", 14),
            "a-very… 2d ago"
        );
        assert_eq!(columns("x", "", 1), "x ");
    }
}
