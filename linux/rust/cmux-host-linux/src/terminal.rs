use gtk4 as gtk;
use gtk::glib;
use gtk::gio::Cancellable;
use vte4::{Pty, PtyFlags, Terminal};
use vte4::prelude::*;

/// Create a new VTE terminal with a shell spawned inside it.
pub fn create_terminal(working_directory: Option<&str>) -> Terminal {
    let terminal = Terminal::new();
    terminal.set_scroll_on_output(true);
    terminal.set_scroll_on_keystroke(true);
    terminal.set_scrollback_lines(10_000);
    terminal.set_hexpand(true);
    terminal.set_vexpand(true);

    // Font
    let font_desc = gtk::pango::FontDescription::from_string("Monospace 11");
    terminal.set_font(Some(&font_desc));

    // Colors — dark terminal background
    let bg = gtk::gdk::RGBA::new(0.09, 0.09, 0.09, 1.0);
    let fg = gtk::gdk::RGBA::new(0.9, 0.9, 0.9, 1.0);
    terminal.set_color_background(&bg);
    terminal.set_color_foreground(&fg);

    // Cursor
    terminal.set_cursor_blink_mode(vte4::CursorBlinkMode::On);
    terminal.set_cursor_shape(vte4::CursorShape::Block);

    // Spawn shell
    spawn_shell(&terminal, working_directory);

    terminal
}

fn spawn_shell(terminal: &Terminal, working_directory: Option<&str>) {
    let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/bash".to_string());

    let pty = match Pty::new_sync(PtyFlags::DEFAULT, None::<&Cancellable>) {
        Ok(pty) => pty,
        Err(e) => {
            eprintln!("Failed to create PTY: {e}");
            return;
        }
    };

    let argv: Vec<&str> = vec![shell.as_str()];
    let envv: Vec<&str> = vec![];
    let working_dir = working_directory.map(|d| d.to_string());

    pty.spawn_async(
        working_dir.as_deref(),
        &argv,
        &envv,
        glib::SpawnFlags::SEARCH_PATH,
        || {},
        -1,
        None::<&Cancellable>,
        |result| {
            if let Err(e) = result {
                eprintln!("Failed to spawn shell: {e}");
            }
        },
    );

    terminal.set_pty(Some(&pty));
}
