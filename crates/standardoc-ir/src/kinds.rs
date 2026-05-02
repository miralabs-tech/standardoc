use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    Function,
    Type,
    Value,
    Module,
    Macro,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum EdgeKind {
    Calls,
    Imports,
    Extends,
    Implements,
    References,
    Defines,
    UsesType,
    ExposesApi,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Visibility {
    Public,
    Private,
    Crate,
    Protected,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceOrigin {
    Workspace,
    CargoRegistry,
    NodeModulesDts,
    ManualExternal,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Language {
    Rust,
    TypeScript,
    JavaScript,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kind_lowercase() {
        assert_eq!(serde_json::to_string(&Kind::Function).unwrap(), "\"function\"");
        assert_eq!(serde_json::to_string(&Kind::Macro).unwrap(), "\"macro\"");
    }

    #[test]
    fn edge_kind_screaming() {
        assert_eq!(serde_json::to_string(&EdgeKind::Calls).unwrap(), "\"CALLS\"");
        assert_eq!(serde_json::to_string(&EdgeKind::UsesType).unwrap(), "\"USES_TYPE\"");
        assert_eq!(serde_json::to_string(&EdgeKind::ExposesApi).unwrap(), "\"EXPOSES_API\"");
    }

    #[test]
    fn visibility_lowercase() {
        assert_eq!(serde_json::to_string(&Visibility::Crate).unwrap(), "\"crate\"");
        assert_eq!(serde_json::to_string(&Visibility::Protected).unwrap(), "\"protected\"");
    }

    #[test]
    fn source_origin_snake() {
        assert_eq!(
            serde_json::to_string(&SourceOrigin::CargoRegistry).unwrap(),
            "\"cargo_registry\""
        );
        assert_eq!(
            serde_json::to_string(&SourceOrigin::NodeModulesDts).unwrap(),
            "\"node_modules_dts\""
        );
        assert_eq!(
            serde_json::to_string(&SourceOrigin::ManualExternal).unwrap(),
            "\"manual_external\""
        );
    }

    #[test]
    fn language_lowercase() {
        assert_eq!(serde_json::to_string(&Language::Rust).unwrap(), "\"rust\"");
        assert_eq!(
            serde_json::to_string(&Language::TypeScript).unwrap(),
            "\"typescript\""
        );
        assert_eq!(
            serde_json::to_string(&Language::JavaScript).unwrap(),
            "\"javascript\""
        );
    }

    #[test]
    fn round_trip_all_kinds() {
        for kind in [
            Kind::Function,
            Kind::Type,
            Kind::Value,
            Kind::Module,
            Kind::Macro,
        ] {
            let s = serde_json::to_string(&kind).unwrap();
            let back: Kind = serde_json::from_str(&s).unwrap();
            assert_eq!(kind, back);
        }
    }

    #[test]
    fn round_trip_all_edge_kinds() {
        for kind in [
            EdgeKind::Calls,
            EdgeKind::Imports,
            EdgeKind::Extends,
            EdgeKind::Implements,
            EdgeKind::References,
            EdgeKind::Defines,
            EdgeKind::UsesType,
            EdgeKind::ExposesApi,
        ] {
            let s = serde_json::to_string(&kind).unwrap();
            let back: EdgeKind = serde_json::from_str(&s).unwrap();
            assert_eq!(kind, back);
        }
    }
}
