//! The console: a terminal that drops down from the top of the screen over whatever is open, and
//! goes away again with the same key.
//!
//! The console is an ordinary toplevel with an app id of its own, and window rules in the config
//! place it, floating along the top. To hide it, umbra takes its tile out of the layout and keeps
//! it, so the window stays mapped and the program in it keeps running. To show it, the same tile
//! goes back on the active workspace.

use crate::layout::ActivateWindow;
use crate::niri::{CastTarget, State};
use crate::utils::spawning::spawn;
use crate::utils::transaction::Transaction;
use crate::utils::with_toplevel_role;
use crate::window::Mapped;

fn is_console(mapped: &Mapped, app_id: &str) -> bool {
    with_toplevel_role(mapped.toplevel(), |role| {
        role.app_id.as_deref() == Some(app_id)
    })
}

impl State {
    /// Shows the console when it is hidden, hides it when it is on the active workspace, brings it
    /// over when it is open on another one, and runs `command` when there is no console yet.
    pub fn toggle_console(&mut self, app_id: &str, command: Vec<String>) {
        if let Some(removed) = self.niri.hidden_console.take() {
            self.show_console_tile(removed);
            return;
        }

        let open = self
            .niri
            .layout
            .windows()
            .find(|(_, mapped)| is_console(mapped, app_id))
            .map(|(_, mapped)| (mapped.window.clone(), mapped.id()));

        if let Some((window, id)) = open {
            let on_active_workspace = self
                .niri
                .layout
                .active_workspace()
                .is_some_and(|ws| ws.has_window(&window));
            let was_active = self.niri.layout.focus().map(|m| &m.window) == Some(&window);

            self.niri
                .stop_casts_for_target(CastTarget::Window { id: id.get() });
            self.niri.window_mru_ui.remove_window(id);
            let removed = self.niri.layout.remove_window(&window, Transaction::new());

            if on_active_workspace {
                self.niri.hidden_console = removed;
                if was_active {
                    self.maybe_warp_cursor_to_focus();
                }
                self.niri.queue_redraw_all();
            } else if let Some(removed) = removed {
                self.show_console_tile(removed);
            }
            return;
        }

        // A console that was just started has a toplevel before it has a buffer. Do not start a
        // second one.
        let starting = self.niri.unmapped_windows.values().any(|unmapped| {
            unmapped.window.toplevel().is_some_and(|toplevel| {
                with_toplevel_role(toplevel, |role| role.app_id.as_deref() == Some(app_id))
            })
        });
        if !starting {
            let (token, _) = self.niri.activation_state.create_external_token(None);
            spawn(command, Some(token.clone()));
        }
    }

    fn show_console_tile(&mut self, removed: crate::layout::RemovedTile<Mapped>) {
        self.niri
            .layout
            .add_removed_tile(removed, ActivateWindow::Yes);
        // The keyboard goes to the console, not to a layer surface that had it on demand.
        self.niri.layer_shell_on_demand_focus = None;
        self.maybe_warp_cursor_to_focus();
        self.niri.queue_redraw_all();
    }
}
