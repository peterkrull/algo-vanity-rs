use std::{io::{self, stdout}, sync::{atomic::{AtomicBool, Ordering}, Arc, Mutex}};
use crossterm::{
    event::{self, Event, KeyCode},
    ExecutableCommand,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen}
};
use ratatui::{prelude::*, widgets::*};
use thousands::Separable;

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
        terminal.draw(|frame|ui_function(frame, state))?;
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

fn ui_function(frame: &mut Frame, state: &Arc<Mutex<GlobalState>>) {
    if let Ok(s) = state.lock() {

        let areas = Layout::new()
            .direction(Direction::Vertical)
            .constraints(
                [
                    Constraint::Max(8),
                    Constraint::Percentage(0),
                    Constraint::Max(1)    
                ])
            .vertical_margin(1)
            .horizontal_margin(2)
            .split(frame.size());

        let sec = s.run_time.as_secs() % 60;
        let min = (s.run_time.as_secs() / 60) % 60;
        let hrs = (s.run_time.as_secs() / 60) / 60;

        let count_message = if s.total_count > 1_000_000_000 {
            format!("Total: {:.2} billion", s.total_count as f32 / 1e9)
        } else {
            format!("Total: {:.2} million", s.total_count as f32 / 1e6)
        };

        let stats_lines = vec![
            Line::raw(format!("Timer: {}h:{:02}m:{:02}s", hrs, min, sec)),
            Line::raw(format!("Speed: {} a/s", (s.search_rate as usize).separate_with_commas())),
            Line::raw(count_message),
            Line::raw(format!("Found: {} matches", s.match_count)),
        ];

        let config_lines = vec![
            Line::raw(format!("Threads:   {}", s.threads)),
            Line::raw(format!("Patterns:  {}", s.vanities.join(", "))),
            Line::raw(format!("Saves to:  {}", s.save_path)),
            Line::raw(format!("Placement: {}", s.placement)),
            // Add more configuration details here...
        ];

        let matches = matches_to_text(&s.matches, (areas[1].height.saturating_sub(4)).into());

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

        let title_matches = match matches.lines.len() {
            0 => String::from(" Matches will appear here "),
            1 => format!(" Last match "),
            _ => format!(" Last {} matches ", matches.lines.len())
        };

        let widget_matches = Paragraph::new(matches)
            .block(Block::default()
                .borders(Borders::ALL)
                .padding(Padding::new(3,3,1,1))
                .title(title_matches)
                .title_style(Style::default().bold())
                .title_alignment(Alignment::Center)
            ).alignment(Alignment::Center);

        let exit_message = Paragraph::new(Text::raw(" Press 'q' to exit "))
            .add_modifier(Modifier::DIM);

        frame.render_widget(widget_stats, areas_top[0]);
        frame.render_widget(widget_config, areas_top[1]);
        frame.render_widget(widget_matches, areas[1]);
        frame.render_widget(exit_message, areas[2]);
    }
}


fn match_to_line(m: &AddressMatch) -> Line {
    // Calculate the start and end of the match
    let (a, b) = match m.placement {
        crate::Placement::Start => (0, m.target.len()),
        crate::Placement::Anywhere(position) => (position, position + m.target.len()),
        crate::Placement::End => (m.public.len() - m.target.len(), m.public.len()),
    };

    // Construct a span with the given text, color and modifier
    let styled_span = |text: &str, color: Color, modifier: Modifier| {
        Span::styled(text.to_owned(), Style::default().fg(color).add_modifier(modifier))
    };

    // Construct the line from the spans
    let spans = vec![
        styled_span(&m.public[ ..a], Color::Gray, Modifier::DIM),
        styled_span(&m.public[a..b], Color::Green, Modifier::BOLD),
        styled_span(&m.public[b.. ], Color::Gray, Modifier::DIM),
    ];

    Line::from(spans)
}

fn matches_to_text(matches: &Vec<AddressMatch>, lines: usize) -> Text {

    // If there are more matches than lines, only draw the last `lines` matches
    let matches_to_draw = &matches[matches.len().saturating_sub(lines)..];

    // Iterate over the matches and render them as lines of text
    matches_to_draw
        .iter()
        .map(match_to_line)
        .collect::<Vec<Line>>()
        .into()
}    