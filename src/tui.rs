use std::{io::{self, stdout}, sync::{atomic::{AtomicBool, Ordering}, Arc, Mutex}};
use crossterm::{
    event::{self, Event, KeyCode},
    ExecutableCommand,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen}
};
use ratatui::{prelude::*, widgets::*};

use crate::{GlobalState, AddressMatch};

pub fn main(
    state: &Arc<Mutex<GlobalState>>,
    keep_alive: Arc<AtomicBool>
) -> io::Result<()> {

    // Setup terminal
    enable_raw_mode()?;
    stdout().execute(EnterAlternateScreen)?;
    let mut terminal = Terminal::new(CrosstermBackend::new(stdout()))?;

    while keep_alive.load(Ordering::Relaxed) {
        terminal.draw(|frame|ui_function_2(frame, state))?;
        handle_events(&keep_alive)?;
    }

    // Tear down terminal
    disable_raw_mode()?;
    stdout().execute(LeaveAlternateScreen)?;
    Ok(())
}

fn handle_events(keep_alive: &Arc<AtomicBool>) -> io::Result<()> {
    if event::poll(std::time::Duration::from_millis(100))? {
        if let Event::Key(key) = event::read()? {
            if key.kind == event::KeyEventKind::Press && key.code == KeyCode::Char('q') {
                keep_alive.store(false, Ordering::Relaxed);
                std::io::stdout().execute(LeaveAlternateScreen).unwrap();
            }
       }
    }
    Ok(())
}

fn ui_function_2(frame: &mut Frame, state: &Arc<Mutex<GlobalState>>) {
    if let Ok(s) = state.lock() {

        let areas = Layout::new()
            .direction(Direction::Vertical)
            .constraints(
                [
                    Constraint::Length(8),
                    Constraint::Length(8),
                ])
            .split(frame.size());

        let sec = s.run_time.as_secs() % 60;
        let min = (s.run_time.as_secs() / 60) % 60;
        let hrs = (s.run_time.as_secs() / 60) / 60;

        let stats_lines = vec![
            Line::raw(format!("Timer: {}h:{:02}m:{:02}s", hrs, min, sec)),
            Line::raw(format!("Speed: {:.0} a/s", s.search_rate)),
            Line::raw(format!("Total: {:.2} million", s.total_count as f32 / 1e6)),
            Line::raw(format!("Match: {}", s.match_count)),
        ];

        let config_lines = vec![
            Line::raw(format!("Threads:   {}", s.threads)),
            Line::raw(format!("Patterns:  {}", s.vanities.join(", "))),
            Line::raw(format!("Saves to:  {}", s.save_path)),
            Line::raw(format!("Placement: {}", s.placement)),
            // Add more configuration details here...
        ];

        let (matches, num_matches) = render_multiple_matches(&s, (areas[1].height.saturating_sub(4)).into());

        let areas_top = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
            .split(areas[0]);

        let widget_stats = Paragraph::new(Text::from(stats_lines))
            .block(Block::default()
                .title(" Session stats ")
                .padding(Padding::new(3,3,1,1))
                .title_style(Style::default().bold())
                .title_position(block::Position::Top)
                .title_alignment(Alignment::Center)
                .borders(Borders::ALL)
            );

        let widget_config = Paragraph::new(Text::from(config_lines))
            .block(Block::default()
                .title(" Configuration ")
                .padding(Padding::new(3,3,1,1))
                .title_style(Style::default().bold())
                .title_position(block::Position::Top)
                .title_alignment(Alignment::Center)
                .borders(Borders::ALL)
            );

        let widget_matches = Paragraph::new(matches)
            .block(Block::default()
                .borders(Borders::ALL)
                .padding(Padding::new(3,3,1,1))
                .title(format!(" Last {} matches ", num_matches))
                .title_style(Style::default().bold())
                .title_alignment(Alignment::Center)
                
            ).alignment(Alignment::Center);

        frame.render_widget(widget_stats, areas_top[0]);
        frame.render_widget(widget_config, areas_top[1]);
        frame.render_widget(widget_matches, areas[1]);
    }
}

fn render_multiple_matches(s: &GlobalState, lines: usize) -> (Text,usize) {
    if s.matches.is_empty() {
        (Text::raw("No matches yet, hang tight.."),0)
    } else if s.matches.len() > lines  {
        (Text::from(s.matches[s.matches.len()-lines..].iter().map(|m| render_stylized_match(m)).collect::<Vec<Line>>()),lines)
    } else if lines > 0 {
        (Text::from(s.matches.iter().map(|m| render_stylized_match(m)).collect::<Vec<Line>>()),s.matches.len())
    } else {
        (Text::raw(""),0)
    }
}

/// Take a single address match and render it in a stylized way, where the matched section
/// is highlighted in bold green, and the rest is regular white text. First 
/// construct each span, and then construct the line from a vec of the spans.
fn render_stylized_match(match_: &AddressMatch) -> Line {
    let mut spans: Vec<Span<'_>> = Vec::new();

    match match_.placement {
        crate::Placement::Start => {
            spans.push(Span::styled(
                &match_.public[0..match_.target.len()],
                Style::default().fg(Color::Green).add_modifier(Modifier::BOLD),
            ));
        
            spans.push(Span::styled(
                &match_.public[match_.target.len()..],
                Style::default().fg(Color::Gray).add_modifier(Modifier::DIM),
            ));
        },
        crate::Placement::Anywhere(position) => {
            spans.push(Span::styled(
                &match_.public[0..position],
                Style::default().fg(Color::Gray).add_modifier(Modifier::DIM),
            ));
            spans.push(Span::styled(
                &match_.public[position..position+match_.target.len()],
                Style::default().fg(Color::Green).add_modifier(Modifier::BOLD),
            ));
            spans.push(Span::styled(
                &match_.public[position+match_.target.len()..],
                Style::default().fg(Color::Gray).add_modifier(Modifier::DIM),
            ));
        },
        crate::Placement::End => {
            spans.push(Span::styled(
                &match_.public[0..match_.public.len()-match_.target.len()],
                Style::default().fg(Color::Gray).add_modifier(Modifier::DIM),
            ));
            spans.push(Span::styled(
                &match_.public[match_.public.len()-match_.target.len()..],
                Style::default().fg(Color::Green).add_modifier(Modifier::BOLD),
            ));
        }
    }

    Line::from(spans)
}

