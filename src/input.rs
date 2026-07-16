use crossterm::event::KeyCode;

use crate::{app::App, models::Screen};

impl App {
    pub(crate) fn handle_key(&mut self, code: KeyCode) {
        if self.input_mode {
            self.handle_input_key(code);
            return;
        }

        match code {
            KeyCode::Char('q') => self.should_quit = true,
            KeyCode::Esc => self.screen = Screen::Welcome,
            KeyCode::Char('1') => self.screen = Screen::Monitor,
            KeyCode::Char('2') => self.screen = Screen::MultiPing,
            KeyCode::Char('3') => self.open_topology(),
            KeyCode::Char('s') => self.scan(),
            KeyCode::Char('a') if self.screen == Screen::MultiPing => {
                self.input_mode = true;
            }
            KeyCode::Char('d') if self.screen == Screen::MultiPing => {
                if self.targets.len() > 1 {
                    self.targets.remove(self.selected_target);
                    self.selected_target = self.selected_target.min(self.targets.len() - 1);
                }
            }
            KeyCode::Up => self.handle_up(),
            KeyCode::Down => self.handle_down(),
            KeyCode::PageUp if self.screen == Screen::Topology => self.scroll_trace_up(5),
            KeyCode::PageDown if self.screen == Screen::Topology => self.scroll_trace_down(5),
            KeyCode::Home if self.screen == Screen::Topology => {
                self.selected_device = 0;
                self.trace_scroll = 0;
            }
            KeyCode::End if self.screen == Screen::Topology => {
                self.selected_device = self.devices.len().saturating_sub(1);
                self.trace_scroll = self.trace_hops.len().saturating_sub(1);
            }
            KeyCode::Enter if self.screen == Screen::Welcome => self.open_selected_welcome_option(),
            _ => {}
        }
    }

    fn handle_input_key(&mut self, code: KeyCode) {
        match code {
            KeyCode::Esc => {
                self.input_mode = false;
                self.input.clear();
            }
            KeyCode::Enter => {
                let host = self.input.trim();
                if !host.is_empty() && !self.targets.iter().any(|target| target.host == host) {
                    self.targets.push(crate::models::PingTarget::new(host));
                    self.selected_target = self.targets.len() - 1;
                }
                self.input_mode = false;
                self.input.clear();
            }
            KeyCode::Backspace => {
                self.input.pop();
            }
            KeyCode::Char(c) => self.input.push(c),
            _ => {}
        }
    }

    fn handle_up(&mut self) {
        match self.screen {
            Screen::Welcome => self.welcome_index = self.welcome_index.saturating_sub(1),
            Screen::MultiPing => {
                self.selected_target = self.selected_target.saturating_sub(1);
            }
            Screen::Topology => self.previous_device(),
            _ => {}
        }
    }

    fn handle_down(&mut self) {
        match self.screen {
            Screen::Welcome => self.welcome_index = (self.welcome_index + 1).min(2),
            Screen::MultiPing => {
                self.selected_target =
                    (self.selected_target + 1).min(self.targets.len().saturating_sub(1));
            }
            Screen::Topology => self.next_device(),
            _ => {}
        }
    }

    fn open_selected_welcome_option(&mut self) {
        self.screen = match self.welcome_index {
            0 => Screen::Monitor,
            1 => Screen::MultiPing,
            _ => Screen::Topology,
        };
        if self.screen == Screen::Topology {
            self.scan();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        app::test_app,
        models::{Device, TraceHop},
    };

    #[test]
    fn keeps_welcome_and_target_navigation_in_bounds() {
        let mut app = test_app();

        app.handle_key(KeyCode::Up);
        assert_eq!(app.welcome_index, 0);
        for _ in 0..5 {
            app.handle_key(KeyCode::Down);
        }
        assert_eq!(app.welcome_index, 2);

        app.screen = Screen::MultiPing;
        app.handle_key(KeyCode::Up);
        assert_eq!(app.selected_target, 0);
        for _ in 0..5 {
            app.handle_key(KeyCode::Down);
        }
        assert_eq!(app.selected_target, 1);
    }

    #[test]
    fn opens_selected_screen_and_quits() {
        let mut app = test_app();
        app.welcome_index = 2;

        app.handle_key(KeyCode::Enter);
        assert!(app.screen == Screen::Topology);
        app.handle_key(KeyCode::Char('1'));
        assert!(app.screen == Screen::Monitor);
        app.handle_key(KeyCode::Char('q'));
        assert!(app.should_quit);
    }

    #[test]
    fn adds_trimmed_unique_target() {
        let mut app = test_app();
        app.screen = Screen::MultiPing;
        app.handle_key(KeyCode::Char('a'));
        for character in "  example.com  ".chars() {
            app.handle_key(KeyCode::Char(character));
        }
        app.handle_key(KeyCode::Enter);

        assert_eq!(app.targets.len(), 3);
        assert_eq!(app.targets[2].host, "example.com");
        assert_eq!(app.selected_target, 2);
        assert!(!app.input_mode);
        assert!(app.input.is_empty());

        app.handle_key(KeyCode::Char('a'));
        for character in "example.com".chars() {
            app.handle_key(KeyCode::Char(character));
        }
        app.handle_key(KeyCode::Enter);
        assert_eq!(app.targets.len(), 3);
    }

    #[test]
    fn cancels_target_input() {
        let mut app = test_app();
        app.screen = Screen::MultiPing;
        app.handle_key(KeyCode::Char('a'));
        app.handle_key(KeyCode::Char('x'));
        app.handle_key(KeyCode::Esc);

        assert!(!app.input_mode);
        assert!(app.input.is_empty());
        assert_eq!(app.targets.len(), 2);
    }

    #[test]
    fn deletes_selected_target_but_keeps_last_target() {
        let mut app = test_app();
        app.screen = Screen::MultiPing;
        app.selected_target = 1;

        app.handle_key(KeyCode::Char('d'));
        assert_eq!(app.targets.len(), 1);
        assert_eq!(app.selected_target, 0);
        app.handle_key(KeyCode::Char('d'));
        assert_eq!(app.targets.len(), 1);
    }

    #[test]
    fn topology_navigation_selects_devices_and_scrolls_trace() {
        let mut app = test_app();
        app.screen = Screen::Topology;
        app.devices = vec![
            Device::arp("192.168.1.10", "aa:bb:cc:dd:ee:ff"),
            Device::arp("192.168.1.20", "11:22:33:44:55:66"),
            Device::arp("192.168.1.30", "22:33:44:55:66:77"),
        ];
        app.trace_hops = (1..=8)
            .map(|hop| TraceHop::new(hop, format!("192.0.2.{hop}"), Some(f64::from(hop))))
            .collect();

        app.handle_key(KeyCode::Down);
        assert_eq!(app.selected_device, 1);
        app.handle_key(KeyCode::Down);
        app.handle_key(KeyCode::Down);
        assert_eq!(app.selected_device, 2);
        app.handle_key(KeyCode::Up);
        assert_eq!(app.selected_device, 1);

        app.handle_key(KeyCode::PageDown);
        assert_eq!(app.trace_scroll, 5);
        app.handle_key(KeyCode::PageUp);
        assert_eq!(app.trace_scroll, 0);
        app.handle_key(KeyCode::End);
        assert_eq!(app.selected_device, 2);
        assert_eq!(app.trace_scroll, 7);
        app.handle_key(KeyCode::Home);
        assert_eq!(app.selected_device, 0);
        assert_eq!(app.trace_scroll, 0);
    }
}
