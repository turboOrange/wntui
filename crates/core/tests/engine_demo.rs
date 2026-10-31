use std::time::Duration;

use wntui_core::{
    engine::{ConnectionState, Engine, EngineCommand, EngineEvent, EngineSource, spawn_engine},
    model::NodeKey,
    session::RetryPolicy,
    wire::metadata::{NodeMetadataPatch, NodeMetadataUpdate},
};

async fn await_live(engine: &Engine) {
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let is_live = {
                let state = engine
                    .state
                    .read()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                state.connection() == ConnectionState::Live && state.store.node_count() == 8
            };

            if is_live {
                return;
            }

            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    })
    .await
    .expect("demo engine never reached live state");
}

#[tokio::test]
async fn demo_engine_reaches_live_with_a_populated_store() {
    let engine = spawn_engine(EngineSource::Demo, RetryPolicy::default());

    await_live(&engine).await;

    let state = engine
        .state
        .read()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let networks = state.networks.clone();
    assert_eq!(networks.len(), 1);
    assert_eq!(networks[0].name, "Demo plant");
    assert!(state.store.loading().is_none());
    assert_eq!(state.store.node_count(), 8);

    engine.task.abort();
}

#[tokio::test]
async fn demo_edit_updates_the_store_and_reports_acceptance() {
    let mut engine = spawn_engine(EngineSource::Demo, RetryPolicy::default());

    await_live(&engine).await;

    let update = NodeMetadataUpdate::new(42, 4)
        .with(NodeMetadataPatch::Name("Line 3 renamed".to_owned()))
        .with(NodeMetadataPatch::Description("renamed by test".to_owned()));

    engine
        .handle
        .send(EngineCommand::SetNodeMetadata(update))
        .expect("engine command channel is open");

    let event = tokio::time::timeout(Duration::from_secs(5), engine.events.recv())
        .await
        .expect("timed out waiting for an engine event")
        .expect("engine event stream closed");

    assert_eq!(
        event,
        EngineEvent::MetadataEditAccepted { address: 4 },
        "demo edits apply locally"
    );

    let state = engine
        .state
        .read()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let node = state
        .store
        .node(&NodeKey::new(42, 4))
        .expect("demo node exists");
    assert_eq!(node.name.as_deref(), Some("Line 3 renamed"));
    assert_eq!(node.description.as_deref(), Some("renamed by test"));

    engine.task.abort();
}

#[tokio::test]
async fn demo_edit_of_a_missing_node_reports_failure() {
    let mut engine = spawn_engine(EngineSource::Demo, RetryPolicy::default());

    let update = NodeMetadataUpdate::new(42, 99).with(NodeMetadataPatch::Name("ghost".to_owned()));

    engine
        .handle
        .send(EngineCommand::SetNodeMetadata(update))
        .expect("engine command channel is open");

    let event = tokio::time::timeout(Duration::from_secs(5), engine.events.recv())
        .await
        .expect("timed out waiting for an engine event")
        .expect("engine event stream closed");

    assert_eq!(
        event,
        EngineEvent::MetadataEditFailed {
            address: 99,
            reason: "node not found".to_owned(),
        }
    );

    engine.task.abort();
}

#[tokio::test]
async fn demo_graph_fetch_answers_with_synthetic_series() {
    let mut engine = spawn_engine(EngineSource::Demo, RetryPolicy::default());

    engine
        .handle
        .send(EngineCommand::FetchGraph {
            key: NodeKey::new(42, 4),
            metric: wntui_core::wire::influx::GraphMetric::Voltage,
            range: wntui_core::wire::influx::GraphRange::Hour24,
            window: None,
        })
        .expect("engine command channel is open");

    let event = tokio::time::timeout(Duration::from_secs(5), engine.events.recv())
        .await
        .expect("timed out waiting for an engine event")
        .expect("engine event stream closed");

    match event {
        EngineEvent::GraphData {
            key,
            metric,
            range,
            window,
            series,
        } => {
            assert_eq!(key, NodeKey::new(42, 4));
            assert_eq!(metric, wntui_core::wire::influx::GraphMetric::Voltage);
            assert_eq!(range, wntui_core::wire::influx::GraphRange::Hour24);
            assert_eq!(window, None, "a range fetch echoes no window");
            assert_eq!(series.len(), 1);
            assert_eq!(series[0].name, "analytics_boot");
            assert_eq!(series[0].points.len(), 48, "24h range uses 48 windows");
            assert!(series[0].points.iter().all(|point| {
                point
                    .values
                    .first()
                    .and_then(|value| value.as_f64())
                    .is_some_and(|value| (2.5..4.0).contains(&value))
            }));
        }
        other => panic!("expected graph data, got {other:?}"),
    }

    // A windowed fetch answers with points inside that window (N4).
    engine
        .handle
        .send(EngineCommand::FetchGraph {
            key: NodeKey::new(42, 4),
            metric: wntui_core::wire::influx::GraphMetric::Voltage,
            range: wntui_core::wire::influx::GraphRange::Hour24,
            window: Some((1_700_000_000, 1_700_000_600)),
        })
        .expect("engine command channel is open");

    let event = tokio::time::timeout(Duration::from_secs(5), engine.events.recv())
        .await
        .expect("timed out waiting for an engine event")
        .expect("engine event stream closed");

    match event {
        EngineEvent::GraphData { window, series, .. } => {
            assert_eq!(window, Some((1_700_000_000, 1_700_000_600)));
            let points = &series[0].points;
            assert!(!points.is_empty());
            assert!(points.iter().all(|point| {
                let seconds = point.timestamp_ns / 1_000_000_000;
                (1_700_000_000..=1_700_000_600).contains(&seconds)
            }));
        }
        other => panic!("expected graph data, got {other:?}"),
    }

    engine.task.abort();
}
