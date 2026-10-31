#[derive(Clone, PartialEq, Eq, Debug)]
pub enum PaletteCommand {
    Quit,
    Help,
    Fields,
    NetworksPicker,
    SelectNetwork { query: String },
    Edit,
    Graph { metric: Option<String> },
    Sort { query: Option<String> },
    FilterSave { name: Option<String> },
    FilterLoad { name: Option<String> },
    FiltersList,
    Unknown(String),
}

pub fn parse_command(input: &str) -> PaletteCommand {
    let input = input.trim();
    let (head, rest) = match input.split_once(char::is_whitespace) {
        Some((head, rest)) => (head, rest.trim()),
        None => (input, ""),
    };

    match head {
        "q" | "quit" => PaletteCommand::Quit,
        "help" | "?" => PaletteCommand::Help,
        "fields" => PaletteCommand::Fields,
        "net" if rest.is_empty() => PaletteCommand::NetworksPicker,
        "net" => PaletteCommand::SelectNetwork {
            query: rest.to_owned(),
        },
        "edit" => PaletteCommand::Edit,
        "graph" => PaletteCommand::Graph {
            metric: (!rest.is_empty()).then(|| rest.to_owned()),
        },
        "sort" => PaletteCommand::Sort {
            query: (!rest.is_empty()).then(|| rest.to_owned()),
        },
        "filter-save" => PaletteCommand::FilterSave {
            name: (!rest.is_empty()).then(|| rest.to_owned()),
        },
        "filter-load" if rest.is_empty() => PaletteCommand::FiltersList,
        "filter-load" => PaletteCommand::FilterLoad {
            name: Some(rest.to_owned()),
        },
        "filters" => PaletteCommand::FiltersList,
        other => PaletteCommand::Unknown(other.to_owned()),
    }
}

pub fn resolve_network<'a>(
    networks: &'a [wntui_core::model::Network],
    query: &str,
) -> Option<&'a wntui_core::model::Network> {
    networks
        .iter()
        .find(|network| network.name.to_lowercase() == query.to_lowercase())
        .or_else(|| {
            networks
                .iter()
                .find(|network| network.name.to_lowercase().contains(&query.to_lowercase()))
        })
        .or_else(|| {
            networks
                .iter()
                .find(|network| network.id.to_string() == query)
        })
        .or_else(|| {
            networks.iter().find(|network| {
                network
                    .id
                    .to_string()
                    .starts_with(query.trim_start_matches("0x"))
            })
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    #[test]
    fn parses_known_commands() {
        assert_eq!(parse_command("q"), PaletteCommand::Quit);
        assert_eq!(parse_command(" quit "), PaletteCommand::Quit);
        assert_eq!(parse_command("help"), PaletteCommand::Help);
        assert_eq!(parse_command("?"), PaletteCommand::Help);
        assert_eq!(parse_command("fields"), PaletteCommand::Fields);
        assert_eq!(parse_command("net"), PaletteCommand::NetworksPicker);
        assert_eq!(
            parse_command("net 42"),
            PaletteCommand::SelectNetwork {
                query: "42".to_owned()
            }
        );
        assert_eq!(parse_command("edit"), PaletteCommand::Edit);
        assert_eq!(
            parse_command("graph"),
            PaletteCommand::Graph { metric: None }
        );
        assert_eq!(
            parse_command("graph voltage"),
            PaletteCommand::Graph {
                metric: Some("voltage".to_owned())
            }
        );
        assert_eq!(parse_command("sort"), PaletteCommand::Sort { query: None });
        assert_eq!(
            parse_command("sort status desc"),
            PaletteCommand::Sort {
                query: Some("status desc".to_owned())
            }
        );
        assert_eq!(
            parse_command("filter-save lab-offline"),
            PaletteCommand::FilterSave {
                name: Some("lab-offline".to_owned())
            }
        );
        assert_eq!(
            parse_command("filter-save"),
            PaletteCommand::FilterSave { name: None }
        );
        assert_eq!(parse_command("filter-load"), PaletteCommand::FiltersList);
        assert_eq!(
            parse_command("filter-load lab-offline"),
            PaletteCommand::FilterLoad {
                name: Some("lab-offline".to_owned())
            }
        );
        assert_eq!(parse_command("filters"), PaletteCommand::FiltersList);
    }

    #[test]
    fn unknown_commands_report_their_head() {
        assert_eq!(
            parse_command("frobnicate now"),
            PaletteCommand::Unknown("frobnicate".to_owned())
        );
    }

    #[test]
    fn network_resolution_prefers_exact_name_then_contains_then_id() {
        let networks = vec![
            wntui_core::model::Network {
                id: 7,
                name: "Lab".to_owned(),
            },
            wntui_core::model::Network {
                id: 42,
                name: "Plant floor".to_owned(),
            },
        ];

        let exact = resolve_network(&networks, "plant floor").unwrap();
        assert_eq!(exact.id, 42);

        let partial = resolve_network(&networks, "plant").unwrap();
        assert_eq!(partial.id, 42);

        let by_id = resolve_network(&networks, "7").unwrap();
        assert_eq!(by_id.id, 7);

        assert!(resolve_network(&networks, "nothing").is_none());
    }
}
