use super::super::resources::{FrameResourceError, FrameResourceResult};

#[test]
fn frame_resource_result_alias_uses_allocator_error() {
    fn pass() -> FrameResourceResult<()> {
        Ok(())
    }

    fn fail() -> FrameResourceResult<()> {
        Err(FrameResourceError::InvalidState {
            reason: "test state",
        })
    }

    assert!(pass().is_ok());
    assert_eq!(
        fail().unwrap_err(),
        FrameResourceError::InvalidState {
            reason: "test state"
        }
    );
}

#[test]
fn frame_resource_error_display_is_diagnostic_friendly() {
    let unsupported = FrameResourceError::Unsupported {
        operation: "resolve",
    };
    assert_eq!(
        unsupported.to_string(),
        "frame resource operation is not supported yet: resolve"
    );

    let invalid_operation = FrameResourceError::InvalidOperation {
        operation: "get_texture",
        reason: "consume phase has not started",
    };
    assert_eq!(
        invalid_operation.to_string(),
        "invalid frame resource operation `get_texture`: consume phase has not started"
    );
}
