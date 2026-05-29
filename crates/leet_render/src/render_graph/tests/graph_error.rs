use leet_core::Leeror;

use super::super::{RenderGraphError, RenderGraphResult};

#[test]
fn render_graph_result_alias_uses_graph_error() {
    fn pass() -> RenderGraphResult<()> {
        Ok(())
    }

    fn fail() -> RenderGraphResult<()> {
        Err(RenderGraphError::InvalidId {
            kind: "node",
            raw: 0,
        })
    }

    assert!(pass().is_ok());
    assert_eq!(
        fail().unwrap_err(),
        RenderGraphError::InvalidId {
            kind: "node",
            raw: 0
        }
    );
}

#[test]
fn render_graph_error_display_is_diagnostic_friendly() {
    assert_eq!(
        RenderGraphError::DuplicateDependency {
            dependency_kind: "CPU",
            parent: 3,
            child: 8,
        }
        .to_string(),
        "duplicate CPU dependency from node 3 to node 8"
    );

    assert_eq!(
        RenderGraphError::InvalidCommandListGroupUsage {
            operation: "create_subnode",
            reason: "no command-list group is open",
        }
        .to_string(),
        "invalid command-list group operation `create_subnode`: no command-list group is open"
    );

    assert_eq!(
        RenderGraphError::CycleDetected {
            dependency_kind: "GPU",
            remaining_nodes: 2,
        }
        .to_string(),
        "GPU dependency cycle detected with 2 unresolved nodes"
    );
}

#[test]
fn render_graph_error_converts_to_leet_error_surface() {
    let error: Leeror = RenderGraphError::InvalidMerge {
        reason: "unique node subtype mismatch",
    }
    .into();

    assert_eq!(
        error.to_string(),
        "Validation error: Render graph error: invalid render graph merge: unique node subtype mismatch"
    );
}
