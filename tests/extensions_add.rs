//! Registration validation for Extensions::add.

mod common;

use common::{Message, MockHandle};
use websocket_extensions::{ExtensionError, Extensions};

#[test]
fn does_not_throw_on_valid_extensions() {
    let mut ext = Extensions::<Message>::new();
    let deflate = MockHandle::new("deflate", true, false, false);
    assert!(ext.add(deflate.extension()).is_ok());
}

#[test]
fn rejects_a_duplicate_name() {
    // Not exercised in the source suite. The source throws a TypeError when a
    // name is already registered.
    let mut ext = Extensions::<Message>::new();
    let a = MockHandle::new("deflate", true, false, false);
    let b = MockHandle::new("deflate", false, true, false);
    assert!(ext.add(a.extension()).is_ok());
    let err = ext.add(b.extension()).unwrap_err();
    assert_eq!(err, ExtensionError::DuplicateName("deflate".to_string()));
    // Pin the rendered message, which a driver may surface to a peer.
    assert_eq!(
        err.to_string(),
        "An extension with name \"deflate\" is already registered"
    );
}

// The source rejects a non-string name and non-boolean RSV fields with a
// TypeError. Here the Extension trait types name as a string and the RSV bits
// as booleans, so those cases cannot be constructed. The type system enforces
// them at compile time.
