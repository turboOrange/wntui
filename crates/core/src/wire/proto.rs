// doc lints fire on comments copied from the wirepas proto files
#[allow(clippy::doc_lazy_continuation, clippy::doc_markdown)]
pub mod wnt {
    include!(concat!(env!("OUT_DIR"), "/com.wirepas.proto.wnt.rs"));
}

// doc lints fire on comments copied from the wirepas proto files
#[allow(clippy::doc_lazy_continuation, clippy::doc_markdown)]
pub mod wpe {
    include!(concat!(env!("OUT_DIR"), "/com.wirepas.proto.wpe.rs"));
}

#[cfg(test)]
mod tests {
    use super::wnt;
    use prost::Message;

    #[test]
    fn message_collection_round_trips_through_generated_types() {
        let collection = wnt::MessageCollection {
            message_collection: vec![wnt::Message {
                network_id: Some(42),
                source_address: Some(0x1a20),
                online_status: Some(wnt::OnlineStatus {
                    status: Some(wnt::online_status::Status::Online as i32),
                    ..Default::default()
                }),
                ..Default::default()
            }],
        };

        let bytes = collection.encode_to_vec();
        let decoded = wnt::MessageCollection::decode(bytes.as_slice()).unwrap();

        let message = decoded.message_collection.first().unwrap();
        assert_eq!(message.network_id, Some(42));
        assert_eq!(message.source_address, Some(0x1a20));
        assert_eq!(
            message.online_status.and_then(|status| status.status),
            Some(wnt::online_status::Status::Online as i32)
        );
    }

    #[test]
    fn diagnostics_carry_voltage_and_role_fields() {
        let collection = wnt::MessageCollection {
            message_collection: vec![wnt::Message {
                network_id: Some(42),
                source_address: Some(7),
                diagnostics: Some(wnt::DiagnosticsData {
                    voltage: Some(3.21),
                    role: Some(wnt::BaseRole::Subnode as i32),
                    ..Default::default()
                }),
                ..Default::default()
            }],
        };

        let decoded =
            wnt::MessageCollection::decode(collection.encode_to_vec().as_slice()).unwrap();
        let diagnostics = decoded.message_collection[0].diagnostics.as_ref().unwrap();

        assert_eq!(diagnostics.voltage, Some(3.21));
        assert_eq!(diagnostics.role, Some(wnt::BaseRole::Subnode as i32));
    }
}
