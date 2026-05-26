use std::sync::Arc;

use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, Paragraph, Wrap};

use super::ui::{centered_rect, spinner_ch};

use super::app::SetupPhase;
use super::registry::ModelEntry;

pub fn draw(f: &mut Frame, phase: &SetupPhase, tick: u16) {
    f.render_widget(
        Block::default().style(Style::default().bg(Color::Black)),
        f.area(),
    );
    match phase {
        SetupPhase::Resolving => draw_spinner(f, "Checking configuration...", tick),
        SetupPhase::FetchingRegistry => draw_spinner(f, "Downloading model catalog...", tick),
        SetupPhase::ModelSelection { entries, selected } => {
            draw_model_selection(f, entries, *selected);
        }
        SetupPhase::DownloadingModel { model_id } => draw_download(f, model_id, tick),
        SetupPhase::VariantSelection {
            model_id,
            variants,
            selected,
        } => draw_variant_selection(f, model_id, variants, *selected),
        SetupPhase::IndexMethodSelection {
            methods, selected, ..
        } => draw_method_selection(f, methods, *selected),
        SetupPhase::Launching {
            model_id, variant, ..
        } => draw_launching(f, model_id, variant, tick),
        SetupPhase::Error {
            message, can_retry, ..
        } => draw_error(f, message, *can_retry),
    }
}

fn draw_spinner(f: &mut Frame, message: &str, tick: u16) {
    let area = centered_rect(50, 20, f.area());
    f.render_widget(Clear, area);

    let block = Block::default()
        .title(" zebraindex setup ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Cyan));

    let text = vec![
        Line::from(""),
        Line::from(vec![
            Span::styled(
                format!("  {} ", spinner_ch(tick)),
                Style::default().fg(Color::Yellow),
            ),
            Span::raw(message),
        ]),
        Line::from(""),
    ];
    let para = Paragraph::new(text).block(block);
    f.render_widget(para, area);
}

fn draw_download(f: &mut Frame, model_id: &str, tick: u16) {
    let area = centered_rect(55, 25, f.area());
    f.render_widget(Clear, area);

    let block = Block::default()
        .title(" zebraindex setup ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Cyan));

    let text = vec![
        Line::from(""),
        Line::from(vec![
            Span::raw("  Model: "),
            Span::styled(model_id, Style::default().fg(Color::Cyan)),
        ]),
        Line::from(""),
        Line::from(vec![
            Span::styled(
                format!("  {} ", spinner_ch(tick)),
                Style::default().fg(Color::Yellow),
            ),
            Span::raw("Downloading from HuggingFace..."),
        ]),
        Line::from(""),
        Line::from(Span::styled(
            "  Please wait, this may take a few minutes.",
            Style::default().fg(Color::DarkGray),
        )),
        Line::from(""),
    ];
    let para = Paragraph::new(text).block(block).wrap(Wrap { trim: false });
    f.render_widget(para, area);
}

fn draw_launching(f: &mut Frame, model_id: &str, variant: &str, tick: u16) {
    let area = centered_rect(50, 20, f.area());
    f.render_widget(Clear, area);

    let block = Block::default()
        .title(" zebraindex setup ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Cyan));

    let text = vec![
        Line::from(""),
        Line::from(vec![
            Span::styled(
                format!("  {} ", spinner_ch(tick)),
                Style::default().fg(Color::Yellow),
            ),
            Span::raw("Starting daemon..."),
        ]),
        Line::from(vec![
            Span::raw("  Model: "),
            Span::styled(model_id, Style::default().fg(Color::Cyan)),
        ]),
        Line::from(vec![
            Span::raw("  Variant: "),
            Span::styled(variant, Style::default().fg(Color::Cyan)),
        ]),
        Line::from(""),
    ];
    let para = Paragraph::new(text).block(block);
    f.render_widget(para, area);
}

fn draw_model_selection(f: &mut Frame, entries: &[ModelEntry], selected: usize) {
    let area = centered_rect(80, 80, f.area());
    f.render_widget(Clear, area);

    let layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(5), Constraint::Length(3)])
        .split(area);

    let mut items: Vec<ListItem> = Vec::with_capacity(entries.len());
    for (i, entry) in entries.iter().enumerate() {
        let is_sel = i == selected;
        let prefix = if is_sel { "> " } else { "  " };
        let style = if is_sel {
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default()
        };

        let tag = if entry.is_downloaded() {
            Span::styled(" [downloaded]", Style::default().fg(Color::Green))
        } else {
            Span::raw("")
        };

        let line1 = Line::from(vec![
            Span::styled(prefix, style),
            Span::styled(&entry.model_id, style),
            Span::styled(
                format!("  ({})", entry.parameters),
                Style::default().fg(Color::DarkGray),
            ),
            tag,
        ]);

        let line2 = Line::from(vec![
            Span::raw("    "),
            Span::styled(&entry.description, Style::default().fg(Color::Gray)),
        ]);

        let techs = entry.technologies.join(", ");
        let line3 = Line::from(vec![
            Span::raw("    "),
            Span::styled(techs, Style::default().fg(Color::DarkGray)),
        ]);

        items.push(ListItem::new(vec![line1, line2, line3]));
    }

    let block = Block::default()
        .title(" Select Embedding Model ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Cyan));
    f.render_widget(List::new(items).block(block), layout[0]);

    let help = Paragraph::new("  j/k: navigate   Enter: select   q: quit")
        .block(Block::default().borders(Borders::ALL));
    f.render_widget(help, layout[1]);
}

fn draw_variant_selection(
    f: &mut Frame,
    model_id: &str,
    variants: &[(Arc<str>, Arc<str>)],
    selected: usize,
) {
    let area = centered_rect(70, 60, f.area());
    f.render_widget(Clear, area);

    let layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(5), Constraint::Length(3)])
        .split(area);

    let mut items: Vec<ListItem> = Vec::with_capacity(variants.len());
    for (i, (name, desc)) in variants.iter().enumerate() {
        let is_sel = i == selected;
        let prefix = if is_sel { "> " } else { "  " };
        let style = if is_sel {
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default()
        };

        let line1 = Line::from(vec![
            Span::styled(prefix, style),
            Span::styled(name.as_ref(), style),
        ]);
        let line2 = Line::from(vec![
            Span::raw("    "),
            Span::styled(desc.as_ref(), Style::default().fg(Color::DarkGray)),
        ]);
        items.push(ListItem::new(vec![line1, line2]));
    }

    let title = format!(" Select ONNX Variant: {} ", model_id);
    let block = Block::default()
        .title(title)
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Cyan));
    f.render_widget(List::new(items).block(block), layout[0]);

    let help = Paragraph::new("  j/k: navigate   Enter: select   Esc: back")
        .block(Block::default().borders(Borders::ALL));
    f.render_widget(help, layout[1]);
}

pub fn draw_method_selection_modal(
    f: &mut Frame,
    methods: &[(zti_ann::SearchMethod, bool)],
    selected: usize,
) {
    draw_method_selection(f, methods, selected);
}

fn draw_method_selection(
    f: &mut Frame,
    methods: &[(zti_ann::SearchMethod, bool)],
    selected: usize,
) {
    let area = centered_rect(90, 85, f.area());
    f.render_widget(Clear, area);

    let outer = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(5), Constraint::Length(3)])
        .split(area);

    let cols = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(35), Constraint::Percentage(65)])
        .split(outer[0]);

    // Left: method list
    let mut items: Vec<ListItem> = Vec::with_capacity(methods.len());
    for (i, &(method, recommended)) in methods.iter().enumerate() {
        let is_sel = i == selected;
        let prefix = if is_sel { "> " } else { "  " };
        let style = if is_sel {
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default()
        };

        let mut spans = vec![Span::styled(prefix, style), Span::styled(method.label(), style)];
        if recommended {
            spans.push(Span::styled(" *", Style::default().fg(Color::Green)));
        }
        items.push(ListItem::new(Line::from(spans)));
    }
    let list_block = Block::default()
        .title(" Index Method ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Cyan));
    f.render_widget(List::new(items).block(list_block), cols[0]);

    // Right: detail panel
    let (method, _) = methods[selected];
    let stats = method.stats();

    let detail_block = Block::default()
        .title(" Method Detail ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Cyan));
    let inner = detail_block.inner(cols[1]);
    f.render_widget(detail_block, cols[1]);

    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Length(6),
            Constraint::Min(6),
            Constraint::Length(4),
        ])
        .split(inner);

    // Title + description
    let title_text = vec![
        Line::from(Span::styled(
            method.label(),
            Style::default()
                .fg(Color::White)
                .add_modifier(Modifier::BOLD),
        )),
        Line::from(Span::styled(
            method.description(),
            Style::default().fg(Color::DarkGray),
        )),
    ];
    f.render_widget(Paragraph::new(title_text), rows[0]);

    // Bar charts
    let bars = vec![
        render_bar("Accuracy", stats.accuracy),
        render_bar("Search Speed", stats.search_speed),
        render_bar("Build Speed", stats.build_speed),
        render_bar("Compression", stats.compression),
    ];
    f.render_widget(Paragraph::new(bars), rows[1]);

    // Parameters
    let mut param_lines: Vec<Line<'_>> = Vec::with_capacity(stats.params.len());
    for &(name, value) in stats.params {
        param_lines.push(Line::from(vec![
            Span::styled(format!(" {:<14}", name), Style::default().fg(Color::White)),
            Span::styled(value, Style::default().fg(Color::Cyan)),
        ]));
    }
    let param_block = Block::default()
        .title(" Parameters ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::DarkGray));
    f.render_widget(Paragraph::new(param_lines).block(param_block), rows[2]);

    // Notes
    let notes = vec![
        Line::from(vec![
            Span::styled("  Best for: ", Style::default().fg(Color::DarkGray)),
            Span::styled(stats.best_for, Style::default().fg(Color::White)),
        ]),
        Line::from(vec![
            Span::styled("  Storage:  ", Style::default().fg(Color::DarkGray)),
            Span::styled(stats.storage_note, Style::default().fg(Color::White)),
        ]),
        Line::from(vec![
            Span::styled("  RAM:      ", Style::default().fg(Color::DarkGray)),
            Span::styled(stats.ram_note, Style::default().fg(Color::White)),
        ]),
    ];
    f.render_widget(Paragraph::new(notes), rows[3]);

    // Help bar
    let help =
        Paragraph::new("  j/k: navigate   Enter: select   a: auto-recommend   Esc: back")
            .block(Block::default().borders(Borders::ALL));
    f.render_widget(help, outer[1]);
}

const FILLED_20: &str = "\u{2588}\u{2588}\u{2588}\u{2588}\u{2588}\u{2588}\u{2588}\u{2588}\u{2588}\u{2588}\u{2588}\u{2588}\u{2588}\u{2588}\u{2588}\u{2588}\u{2588}\u{2588}\u{2588}\u{2588}";
const EMPTY_20: &str = "\u{2591}\u{2591}\u{2591}\u{2591}\u{2591}\u{2591}\u{2591}\u{2591}\u{2591}\u{2591}\u{2591}\u{2591}\u{2591}\u{2591}\u{2591}\u{2591}\u{2591}\u{2591}\u{2591}\u{2591}";

fn bar_slice(full: &'static str, n: usize) -> &'static str {
    let byte_len = full.len() / 20 * n;
    &full[..byte_len]
}

fn render_bar(label: &'static str, pct: u8) -> Line<'static> {
    let filled = (pct as usize).saturating_mul(20) / 100;
    let empty = 20 - filled;
    let color = match pct {
        90..=100 => Color::Green,
        60..=89 => Color::Cyan,
        30..=59 => Color::Yellow,
        _ => Color::Red,
    };
    Line::from(vec![
        Span::styled(
            format!("  {:<14}", label),
            Style::default().fg(Color::White),
        ),
        Span::styled(bar_slice(FILLED_20, filled), Style::default().fg(color)),
        Span::styled(
            bar_slice(EMPTY_20, empty),
            Style::default().fg(Color::DarkGray),
        ),
        Span::styled(format!("  {:>3}%", pct), Style::default().fg(Color::White)),
    ])
}

fn draw_error(f: &mut Frame, message: &str, can_retry: bool) {
    let area = centered_rect(50, 25, f.area());
    f.render_widget(Clear, area);

    let keys = if can_retry {
        "  r: retry   q: quit"
    } else {
        "  q: quit"
    };

    let block = Block::default()
        .title(" Error ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Red));

    let text = vec![
        Line::from(""),
        Line::from(Span::styled(
            format!("  {}", message),
            Style::default().fg(Color::Red),
        )),
        Line::from(""),
        Line::from(Span::styled(keys, Style::default().fg(Color::DarkGray))),
        Line::from(""),
    ];

    let para = Paragraph::new(text)
        .block(block)
        .wrap(Wrap { trim: false });
    f.render_widget(para, area);
}
