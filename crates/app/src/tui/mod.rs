use std::{io, time::Duration};

use anyhow::Context;
use crossterm::{
    event::{DisableMouseCapture, EnableMouseCapture, Event, EventStream, KeyEventKind},
    execute,
};
use futures_util::StreamExt;
use tokio::sync::mpsc;
use wntui_core::{
    config::{Config, Profile, config_path as default_config_path},
    engine::{Engine, EngineCommand, EngineEvent, EngineHandle, EngineSource, spawn_engine},
    session::{Credentials, RetryPolicy},
};

mod app;
mod complete;
mod detail;
mod graphs;
mod input;
mod listing;
mod palette;
mod theme;
mod update;
mod view;

use app::AppState;

const RENDER_INTERVAL: Duration = Duration::from_millis(50);

fn milliseconds_since_epoch() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or(0)
}

enum Msg {
    Key(ratatui::crossterm::event::KeyEvent),
    Mouse(ratatui::crossterm::event::MouseEvent),
    Engine(EngineEvent),
    Tick,
}

pub fn run_demo() -> anyhow::Result<()> {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;

    runtime.block_on(async {
        let engine = spawn_engine(EngineSource::Demo, RetryPolicy::default());
        run_loop(engine).await
    })
}

pub fn run(
    profile_name: Option<&str>,
    config_path: Option<&std::path::Path>,
) -> anyhow::Result<()> {
    let path = match config_path {
        Some(path) => path.to_path_buf(),
        None => default_config_path().context("cannot determine the config directory")?,
    };

    let config = Config::load(&path).with_context(|| {
        format!(
            "cannot load config from {} — create it or run `wntui --demo`",
            path.display()
        )
    })?;
    let profile = config.resolve_profile(profile_name)?;
    let profile_name = profile_name
        .map(str::to_owned)
        .or_else(|| config.default_profile.clone())
        .ok_or_else(|| anyhow::anyhow!("no profile given and no default_profile set"))?;

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;

    let credentials = resolve_credentials(&profile_name, profile)?;
    let saved_filters = config.filters.clone();

    runtime.block_on(async {
        let source = live_source(profile, credentials);
        let engine = spawn_engine(source, RetryPolicy::default());
        run_loop_with_filters(engine, saved_filters, Some(path)).await
    })
}

fn live_source(profile: &Profile, credentials: Credentials) -> EngineSource {
    EngineSource::live(profile, credentials)
}

// Same resolution order as the CLI: WNT_PASSWORD env, then the OS keyring
// (see `wntui auth login`), then an interactive prompt.
fn resolve_credentials(profile_name: &str, profile: &Profile) -> anyhow::Result<Credentials> {
    let password = match std::env::var("WNT_PASSWORD") {
        Ok(password) => password,
        Err(_) => match wntui_core::secrets::load(profile_name, profile) {
            Ok(Some(password)) => password,
            Ok(None) => rpassword::prompt_password(format!(
                "WNT password for {}@{}: ",
                profile.username, profile.host
            ))
            .context("no password given")?,
            Err(error) => {
                eprintln!("warning: keyring unavailable ({error}) — prompting instead");
                rpassword::prompt_password(format!(
                    "WNT password for {}@{}: ",
                    profile.username, profile.host
                ))
                .context("no password given")?
            }
        },
    };

    Ok(Credentials {
        username: profile.username.to_owned(),
        password,
    })
}

async fn run_loop(engine: Engine) -> anyhow::Result<()> {
    run_loop_with_filters(engine, Default::default(), None).await
}

async fn run_loop_with_filters(
    engine: Engine,
    saved_filters: std::collections::BTreeMap<String, String>,
    config_path: Option<std::path::PathBuf>,
) -> anyhow::Result<()> {
    let mut terminal = ratatui::init();
    execute!(io::stdout(), EnableMouseCapture)?;

    let (state, handle, task, engine_events) = engine.into_parts();

    let (messages, mut message_box) = mpsc::channel(64);
    tokio::spawn(feed_terminal_events(messages.clone()));
    tokio::spawn(feed_ticks(messages.clone()));
    tokio::spawn(feed_engine_events(engine_events, messages));

    let result = drive(
        &state,
        &handle,
        &mut terminal,
        &mut message_box,
        &saved_filters,
        config_path.as_deref(),
    )
    .await;

    execute!(io::stdout(), DisableMouseCapture)?;
    ratatui::restore();
    task.abort();

    result
}

async fn drive(
    engine_state: &wntui_core::engine::SharedEngineState,
    handle: &wntui_core::engine::EngineHandle,
    terminal: &mut ratatui::DefaultTerminal,
    message_box: &mut mpsc::Receiver<Msg>,
    saved_filters: &std::collections::BTreeMap<String, String>,
    config_path: Option<&std::path::Path>,
) -> anyhow::Result<()> {
    let mut application = AppState::new();
    application.saved_filters = saved_filters.clone();

    // The config snapshot at launch; filter saves rewrite it and persist.
    let mut config = Config::default();
    let mut have_config = false;
    if let Some(path) = config_path {
        match Config::load(path) {
            Ok(loaded) => {
                config = loaded;
                have_config = true;
            }
            Err(error) => {
                application.message = Some(format!(
                    "filter profiles disabled: cannot load {} ({error})",
                    path.display()
                ));
            }
        }
    }

    loop {
        application.now_ms = milliseconds_since_epoch();
        application.page_height = terminal
            .size()
            .map(|size| size.height.saturating_sub(4) as usize)
            .unwrap_or(20)
            .max(1);

        {
            let state = engine_state
                .read()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            // The projection cache recomputes only when the store version,
            // filter, sort, or network moved; the render path is a pure
            // read of the cached keys (N2).
            update::refresh_projection(&mut application, &state);
            terminal.draw(|frame| view::render(&application, &state, frame))?;
        }

        let Some(message) = message_box.recv().await else {
            break;
        };

        let action = match message {
            Msg::Key(key)
                if key.kind == KeyEventKind::Press || key.kind == KeyEventKind::Repeat =>
            {
                input::action_for(&application, key)
            }
            Msg::Key(_) => input::Action::Noop,
            Msg::Mouse(mouse) => input::scroll_action(mouse),
            Msg::Engine(event) => {
                application.apply_engine_event(event.clone());
                if let Some(message) = engine_event_message(&event) {
                    application.message = Some(message);
                }
                continue;
            }
            Msg::Tick => input::Action::Noop,
        };

        let outcome = {
            let state = engine_state
                .read()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            update::update(&mut application, action, &state)
        };

        forward_pending_edit(&mut application, handle);
        forward_pending_graph(&mut application, handle);
        persist_filter_save(&mut application, &mut config, config_path, have_config);

        if outcome == update::Outcome::Quit || application.quit {
            break;
        }
    }

    Ok(())
}

// `:filter-save` lands in pending_filter_save; this drains it to the
// config file. The in-memory AppState copy is already updated by the
// reducer, so a failed write only costs persistence, not the session.
fn persist_filter_save(
    application: &mut AppState,
    config: &mut Config,
    config_path: Option<&std::path::Path>,
    have_config: bool,
) {
    let Some((name, expression)) = application.pending_filter_save.take() else {
        return;
    };

    let Some(path) = config_path else {
        application.message =
            Some("filter profiles need a config file — start with --config".to_owned());
        return;
    };

    if !have_config {
        application.message =
            Some("filter profiles disabled: the config file could not be loaded".to_owned());
        return;
    }

    config.filters.insert(name.clone(), expression);
    match config.save(path) {
        Ok(()) => application.message = Some(format!("filter saved as '{name}'")),
        Err(error) => application.message = Some(format!("cannot save the config: {error}")),
    }
}

fn engine_event_message(event: &EngineEvent) -> Option<String> {
    match event {
        EngineEvent::MetadataEditAccepted { address } => Some(format!("node {address} updated")),
        EngineEvent::MetadataEditFailed { address, reason } => {
            Some(format!("node {address} edit failed: {reason}"))
        }
        // Graph replies land in the graph request state, not the status bar.
        EngineEvent::GraphData { .. } | EngineEvent::GraphFailed { .. } => None,
    }
}

fn forward_pending_edit(application: &mut AppState, handle: &EngineHandle) {
    let Some(edit) = application.pending_edit.take() else {
        return;
    };

    if handle.send(EngineCommand::SetNodeMetadata(edit)).is_err() {
        application.message = Some("the engine is shutting down — edit dropped".to_owned());
    }
}

fn forward_pending_graph(application: &mut AppState, handle: &EngineHandle) {
    let Some(request) = application.pending_graph.take() else {
        return;
    };

    let command = EngineCommand::FetchGraph {
        key: request.key,
        metric: request.metric,
        range: request.range,
        window: request.window,
    };

    if handle.send(command).is_err()
        && let Some(graph) = application.graph.as_mut()
    {
        graph.loading = false;
        graph.error = Some("the engine is shutting down".to_owned());
    }
}

async fn feed_terminal_events(sender: mpsc::Sender<Msg>) {
    let mut stream = EventStream::new();

    while let Some(event) = stream.next().await {
        let message = match event {
            Ok(Event::Key(key)) => Msg::Key(key),
            Ok(Event::Mouse(mouse)) => Msg::Mouse(mouse),
            Ok(_) => continue,
            Err(_) => break,
        };

        if sender.send(message).await.is_err() {
            break;
        }
    }
}

async fn feed_engine_events(
    mut events: tokio::sync::mpsc::Receiver<EngineEvent>,
    sender: mpsc::Sender<Msg>,
) {
    while let Some(event) = events.recv().await {
        if sender.send(Msg::Engine(event)).await.is_err() {
            break;
        }
    }
}

async fn feed_ticks(sender: mpsc::Sender<Msg>) {
    let mut interval = tokio::time::interval(RENDER_INTERVAL);
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    loop {
        interval.tick().await;
        if sender.send(Msg::Tick).await.is_err() {
            break;
        }
    }
}
