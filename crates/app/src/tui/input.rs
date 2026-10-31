use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use super::app::{AppState, Overlay, Page};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Action {
    MoveDown,
    MoveUp,
    ToTop,
    ToBottom,
    HalfPageDown,
    HalfPageUp,
    FullPageDown,
    FullPageUp,
    Enter,
    Back,
    OpenPalette,
    OpenHelp,
    OpenNetworks,
    OpenFilter,
    CycleEditField,
    OpenGraphs,
    CycleGraphRange,
    PanGraphLeft,
    PanGraphRight,
    ZoomInGraph,
    ZoomOutGraph,
    GraphJumpStart,
    GraphJumpEnd,
    RefetchGraph,
    CycleSort,
    ReverseSort,
    ScrollUp,
    ScrollDown,
    TextualInput(char),
    TextualBackspace,
    TextualSubmit,
    TextualCancel,
    CompleteFilter,
    Quit,
    Noop,
}

pub fn action_for(state: &AppState, event: KeyEvent) -> Action {
    if event.modifiers.contains(KeyModifiers::CONTROL) {
        return match event.code {
            KeyCode::Char('c') => Action::Quit,
            KeyCode::Char('d') => Action::HalfPageDown,
            KeyCode::Char('u') => Action::HalfPageUp,
            KeyCode::Char('f') => Action::FullPageDown,
            KeyCode::Char('b') => Action::FullPageUp,
            _ => Action::Noop,
        };
    }

    match state.overlay {
        Some(Overlay::Palette) => match event.code {
            KeyCode::Char(character) => Action::TextualInput(character),
            KeyCode::Backspace => Action::TextualBackspace,
            KeyCode::Enter => Action::TextualSubmit,
            KeyCode::Esc => Action::TextualCancel,
            _ => Action::Noop,
        },
        Some(Overlay::Filter) => match event.code {
            KeyCode::Char(character) => Action::TextualInput(character),
            KeyCode::Backspace => Action::TextualBackspace,
            KeyCode::Enter => Action::TextualSubmit,
            KeyCode::Esc => Action::TextualCancel,
            KeyCode::Tab => Action::CompleteFilter,
            _ => Action::Noop,
        },
        Some(Overlay::Fields) | Some(Overlay::Help) => match event.code {
            KeyCode::Char('?') | KeyCode::Esc | KeyCode::Char('q') => Action::Back,
            _ => Action::Noop,
        },
        Some(Overlay::Edit(_)) => match event.code {
            KeyCode::Char(character) => Action::TextualInput(character),
            KeyCode::Backspace => Action::TextualBackspace,
            KeyCode::Enter => Action::TextualSubmit,
            KeyCode::Esc => Action::TextualCancel,
            KeyCode::Tab => Action::CycleEditField,
            _ => Action::Noop,
        },
        None => match state.page {
            Page::Nodes => nodes_action(event),
            Page::Networks => networks_action(event),
            Page::NodeDetail => detail_action(event),
            Page::Graphs => graphs_action(event),
        },
    }
}

fn detail_action(event: KeyEvent) -> Action {
    match event.code {
        KeyCode::Char('j') | KeyCode::Down => Action::MoveDown,
        KeyCode::Char('k') | KeyCode::Up => Action::MoveUp,
        KeyCode::Char('g') | KeyCode::Home => Action::ToTop,
        KeyCode::Char('G') | KeyCode::End => Action::ToBottom,
        KeyCode::Enter => Action::Enter,
        KeyCode::Esc | KeyCode::Char('q') => Action::Back,
        KeyCode::Char(':') => Action::OpenPalette,
        KeyCode::Char('?') => Action::OpenHelp,
        KeyCode::Char('v') => Action::OpenGraphs,
        _ => Action::Noop,
    }
}

fn graphs_action(event: KeyEvent) -> Action {
    match event.code {
        KeyCode::Esc | KeyCode::Char('q') => Action::Back,
        KeyCode::Char(':') => Action::OpenPalette,
        KeyCode::Char('?') => Action::OpenHelp,
        KeyCode::Char('t') => Action::CycleGraphRange,
        // Pan and zoom (N4): h/l and arrows pan, +/− zoom, H/L jump to
        // the fetched edges, r refetches the current window.
        KeyCode::Char('h') | KeyCode::Left => Action::PanGraphLeft,
        KeyCode::Char('l') | KeyCode::Right => Action::PanGraphRight,
        KeyCode::Char('+') | KeyCode::Char('=') => Action::ZoomInGraph,
        KeyCode::Char('-') | KeyCode::Char('_') => Action::ZoomOutGraph,
        KeyCode::Char('H') => Action::GraphJumpStart,
        KeyCode::Char('L') => Action::GraphJumpEnd,
        KeyCode::Char('r') => Action::RefetchGraph,
        KeyCode::Char('j') | KeyCode::Down => Action::MoveDown,
        KeyCode::Char('k') | KeyCode::Up => Action::MoveUp,
        _ => Action::Noop,
    }
}

fn nodes_action(event: KeyEvent) -> Action {
    match event.code {
        KeyCode::Char('j') | KeyCode::Down => Action::MoveDown,
        KeyCode::Char('k') | KeyCode::Up => Action::MoveUp,
        KeyCode::Char('g') | KeyCode::Home => Action::ToTop,
        KeyCode::Char('G') | KeyCode::End => Action::ToBottom,
        KeyCode::Enter => Action::Enter,
        KeyCode::Esc | KeyCode::Char('q') => Action::Quit,
        KeyCode::Char(':') => Action::OpenPalette,
        KeyCode::Char('?') => Action::OpenHelp,
        KeyCode::Char('n') => Action::OpenNetworks,
        KeyCode::Char('/') => Action::OpenFilter,
        KeyCode::Char('s') => Action::CycleSort,
        KeyCode::Char('S') => Action::ReverseSort,
        _ => Action::Noop,
    }
}

fn networks_action(event: KeyEvent) -> Action {
    match event.code {
        KeyCode::Char('j') | KeyCode::Down => Action::MoveDown,
        KeyCode::Char('k') | KeyCode::Up => Action::MoveUp,
        KeyCode::Char('g') | KeyCode::Home => Action::ToTop,
        KeyCode::Char('G') | KeyCode::End => Action::ToBottom,
        KeyCode::Enter => Action::Enter,
        KeyCode::Esc | KeyCode::Char('q') => Action::Back,
        KeyCode::Char(':') => Action::OpenPalette,
        KeyCode::Char('?') => Action::OpenHelp,
        KeyCode::Backspace => Action::TextualBackspace,
        KeyCode::Char(character) => Action::TextualInput(character),
        _ => Action::Noop,
    }
}

// The drive loop maps mouse events; wheel pans the graph window and
// ctrl+wheel zooms (N4). ScrollUp/ScrollDown drive the node list on the
// other pages.
pub fn scroll_action(mouse: crossterm::event::MouseEvent) -> Action {
    use crossterm::event::MouseEventKind;

    match mouse.kind {
        MouseEventKind::ScrollUp => {
            if mouse.modifiers.contains(KeyModifiers::CONTROL) {
                Action::ZoomInGraph
            } else {
                Action::ScrollUp
            }
        }
        MouseEventKind::ScrollDown => {
            if mouse.modifiers.contains(KeyModifiers::CONTROL) {
                Action::ZoomOutGraph
            } else {
                Action::ScrollDown
            }
        }
        _ => Action::Noop,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tui::app::EditField;
    use crossterm::event::{KeyCode, KeyEvent, MouseEventKind};
    use pretty_assertions::assert_eq;

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn detail_state() -> AppState {
        AppState {
            page: Page::NodeDetail,
            ..AppState::new()
        }
    }

    #[test]
    fn detail_keys_map_to_navigation_and_activation() {
        let state = detail_state();
        assert_eq!(
            action_for(&state, key(KeyCode::Char('j'))),
            Action::MoveDown
        );
        assert_eq!(
            action_for(&state, key(KeyCode::Enter)),
            Action::Enter,
            "enter activates the field under the cursor"
        );
        assert_eq!(
            action_for(&state, key(KeyCode::Char('e'))),
            Action::Noop,
            "e no longer opens the editor directly"
        );
        assert_eq!(action_for(&state, key(KeyCode::Char('q'))), Action::Back);
    }

    #[test]
    fn detail_and_graphs_keys_map_to_graph_actions() {
        let detail = detail_state();
        assert_eq!(
            action_for(&detail, key(KeyCode::Char('v'))),
            Action::OpenGraphs
        );
        assert_eq!(
            action_for(&detail, key(KeyCode::Char('t'))),
            Action::Noop,
            "t is graphs-only"
        );

        let graphs = AppState {
            page: Page::Graphs,
            ..AppState::new()
        };
        assert_eq!(
            action_for(&graphs, key(KeyCode::Char('t'))),
            Action::CycleGraphRange
        );
        assert_eq!(action_for(&graphs, key(KeyCode::Char('q'))), Action::Back);
        assert_eq!(
            action_for(&graphs, key(KeyCode::Char('v'))),
            Action::Noop,
            "v is detail-only"
        );
    }

    #[test]
    fn graphs_keys_map_to_pan_and_zoom_actions() {
        let graphs = AppState {
            page: Page::Graphs,
            ..AppState::new()
        };

        assert_eq!(
            action_for(&graphs, key(KeyCode::Char('h'))),
            Action::PanGraphLeft
        );
        assert_eq!(
            action_for(&graphs, key(KeyCode::Left)),
            Action::PanGraphLeft
        );
        assert_eq!(
            action_for(&graphs, key(KeyCode::Char('l'))),
            Action::PanGraphRight
        );
        assert_eq!(
            action_for(&graphs, key(KeyCode::Char('+'))),
            Action::ZoomInGraph
        );
        assert_eq!(
            action_for(&graphs, key(KeyCode::Char('='))),
            Action::ZoomInGraph
        );
        assert_eq!(
            action_for(&graphs, key(KeyCode::Char('-'))),
            Action::ZoomOutGraph
        );
        assert_eq!(
            action_for(&graphs, key(KeyCode::Char('H'))),
            Action::GraphJumpStart
        );
        assert_eq!(
            action_for(&graphs, key(KeyCode::Char('L'))),
            Action::GraphJumpEnd
        );
        assert_eq!(
            action_for(&graphs, key(KeyCode::Char('r'))),
            Action::RefetchGraph
        );
    }

    #[test]
    fn wheel_pans_and_ctrl_wheel_zooms() {
        let scroll = |kind, modifiers| crossterm::event::MouseEvent {
            kind,
            column: 0,
            row: 0,
            modifiers,
        };

        assert_eq!(
            scroll_action(scroll(MouseEventKind::ScrollUp, KeyModifiers::NONE)),
            Action::ScrollUp
        );
        assert_eq!(
            scroll_action(scroll(MouseEventKind::ScrollUp, KeyModifiers::CONTROL)),
            Action::ZoomInGraph
        );
        assert_eq!(
            scroll_action(scroll(MouseEventKind::ScrollDown, KeyModifiers::CONTROL)),
            Action::ZoomOutGraph
        );
    }

    #[test]
    fn edit_overlay_keys_are_textual_with_tab_switching() {
        let state = AppState {
            overlay: Some(Overlay::Edit(EditField::Name)),
            ..AppState::new()
        };

        assert_eq!(
            action_for(&state, key(KeyCode::Char('x'))),
            Action::TextualInput('x')
        );
        assert_eq!(
            action_for(&state, key(KeyCode::Backspace)),
            Action::TextualBackspace
        );
        assert_eq!(
            action_for(&state, key(KeyCode::Enter)),
            Action::TextualSubmit
        );
        assert_eq!(action_for(&state, key(KeyCode::Esc)), Action::TextualCancel);
        assert_eq!(
            action_for(&state, key(KeyCode::Tab)),
            Action::CycleEditField
        );
    }

    #[test]
    fn nodes_page_keeps_its_existing_keymap() {
        let state = AppState::new();
        assert_eq!(
            action_for(&state, key(KeyCode::Char('/'))),
            Action::OpenFilter
        );
        assert_eq!(
            action_for(&state, key(KeyCode::Char('s'))),
            Action::CycleSort
        );
    }
}
