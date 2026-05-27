use std::fmt::Write;

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders};

use super::super::app::{App, DaemonStatus};

pub fn draw_status_bar(f: &mut Frame, app: &App, area: Rect) {
    let color = match &app.daemon_status {
        DaemonStatus::Running { .. } => Color::Green,
        DaemonStatus::Starting => Color::Yellow,
        DaemonStatus::Error(_) => Color::Red,
        _ => Color::DarkGray,
    };

    let indicator = match &app.daemon_status {
        DaemonStatus::Unknown => "?",
        DaemonStatus::Starting => "\u{25CC}",
        DaemonStatus::Running { .. } => "\u{25CF}",
        DaemonStatus::Stopped => "\u{25CB}",
        DaemonStatus::Error(_) => "!",
    };

    let status_label = match &app.daemon_status {
        DaemonStatus::Unknown => "Unknown",
        DaemonStatus::Starting => "Starting",
        DaemonStatus::Running { .. } => "Running",
        DaemonStatus::Stopped => "Stopped",
        DaemonStatus::Error(_) => "Error",
    };

    let model = app.model.as_deref().unwrap_or("--");
    let dtype = app.model_dtype.as_deref().unwrap_or("--");

    let uptime_str = match &app.daemon_status {
        DaemonStatus::Running { uptime_secs, .. } => {
            let mins = uptime_secs / 60;
            let hrs = mins / 60;
            if hrs > 0 {
                Some(format!("{}h {}m", hrs, mins % 60))
            } else {
                Some(format!("{}m", mins))
            }
        }
        _ => None,
    };

    let (device, cpus, mem_total_mb) = app.effective_hardware();

    let mut text = String::with_capacity(160);
    if let DaemonStatus::Error(err_msg) = &app.daemon_status {
        let first_line = err_msg.lines().next().unwrap_or(err_msg.as_str());
        write!(text, "{}  Error: {}  (see daemon.log)", indicator, first_line).ok();
    } else {
        write!(
            text,
            "{}  {}  Model: {}  DType: {}  Device: {}  CPU: {}",
            indicator, status_label, model, dtype, device, cpus,
        )
        .ok();

        if mem_total_mb > 0 {
            write!(text, "  RAM: {}M", mem_total_mb).ok();
        }
        if let Some(uptime) = &uptime_str {
            write!(text, "  {}", uptime).ok();
        }
    }

    let line = Line::from(vec![Span::styled(text, Style::default().fg(color))]);
    let block = Block::default().title(" zebraindex ").borders(Borders::ALL);
    let para = ratatui::widgets::Paragraph::new(line).block(block);
    f.render_widget(para, area);
}
