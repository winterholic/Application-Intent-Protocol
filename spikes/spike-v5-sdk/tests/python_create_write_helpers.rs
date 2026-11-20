//! The Python binding has the typed create and write-extension helpers the TypeScript binding has (`bundle` create, `writeExtension`).
use spike_v1_fixture::{load_str, Form};
use spike_v2_read::id_wire::IdWire;
use std::io::Write;
use std::process::{Command, Stdio};

const SOURCE: &str = r#"
resource Member { fields { id: Id } }
actor Member
resource Log {
  fields { id: Id; owner: Member; name: Text(1..50); note: Text?; touched: Bool }
  rows read when owner = actor
  transition touch { allow owner = actor; from touched = false; to touched = true }
  expose apply touch { target id; bulk maxRows 2 }
  expose create { allow owner = actor; fields owner, name, note, touched }
  extension write touchAll {
    input { id: Log.Id; reason: Text? }
    output { id: Log.Id; count: Int }
    access Log.touch
    effect db
    deadline 2s
    implementation "log.touch_all"
  }
  extension write hidden {
    input { id: Log.Id }
    output { count: Int }
    access Log.touch
    effect db
    deadline 2s
    internal
    implementation "log.hidden"
  }
}
"#;

#[test]
fn create_and_write_extension_helpers_are_generated_and_delegate_to_the_client() {
    let facts = load_str(SOURCE, Form::A).unwrap_or_else(|errors| panic!("{errors:?}")).execution;
    for wire in [IdWire::SafeNumber, IdWire::DecimalString] {
        let generated = spike_v5_sdk::contract_module_python(&facts, wire);
        for expected in [
            "LogCreateValues = TypedDict(\"LogCreateValues\"",
            "\"note\": NotRequired[Optional[str]]",
            "def create_log(client: Client, values: LogCreateValues, *, key: str | None = None) -> dict:",
            "LogTouchAllWriteInput = TypedDict(\"LogTouchAllWriteInput\"",
            "\"reason\": NotRequired[Optional[str]]",
            "LogTouchAllWriteOutput = TypedDict(\"LogTouchAllWriteOutput\"",
            "def write_log_touch_all(client: Client, input: LogTouchAllWriteInput, *, key: str | None = None) -> dict:",
        ] {
            assert!(generated.contains(expected), "{wire:?}: missing {expected}\n{generated}");
        }
        assert!(!generated.contains("LogHidden") && !generated.contains("Log.hidden"), "an internal extension is not part of the caller contract");
        let script = r#"
import sys
sys.path.insert(0, sys.argv[1])
scope = {}
exec(sys.stdin.read(), scope)
class Stub:
    def __init__(self): self.calls = []
    def create(self, resource, values, key=None): self.calls.append(("create", resource, values, key)); return {"ok": True}
    def write_extension(self, name, input, key=None): self.calls.append(("write_extension", name, input, key)); return {"ok": True}
stub = Stub()
scope["create_log"](stub, {"owner": "1", "name": "n", "touched": False}, key="c1")
scope["write_log_touch_all"](stub, {"id": "9"}, key="w1")
assert stub.calls == [("create", "Log", {"owner": "1", "name": "n", "touched": False}, "c1"), ("write_extension", "Log.touchAll", {"id": "9"}, "w1")], stub.calls
"#;
        let sdk = concat!(env!("CARGO_MANIFEST_DIR"), "/../../product/python");
        let mut child = Command::new("python3").args(["-c", script, sdk]).stdin(Stdio::piped()).stderr(Stdio::piped()).spawn().expect("python3");
        child.stdin.take().unwrap().write_all(generated.as_bytes()).unwrap();
        let out = child.wait_with_output().unwrap();
        assert!(out.status.success(), "{wire:?}: {}", String::from_utf8_lossy(&out.stderr));
    }
}

#[test]
fn a_definition_without_creates_or_write_extensions_generates_no_new_helper() {
    let facts = load_str("resource Member { fields { id: Id } }\nactor Member\n", Form::A).unwrap_or_else(|errors| panic!("{errors:?}")).execution;
    let generated = spike_v5_sdk::contract_module_python(&facts, IdWire::DecimalString);
    assert!(!generated.contains("def create_") && !generated.contains("def write_"), "{generated}");
}
