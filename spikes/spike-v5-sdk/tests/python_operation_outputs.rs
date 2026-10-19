use spike_v1_fixture::{load_str, Form};
use spike_v2_read::id_wire::IdWire;
use std::io::Write;
use std::process::{Command, Stdio};

const SOURCE: &str = r#"
actor principal
enum State { READY, DONE }
resource Document { fields { id: Uuid } }
operation read inspect {
  input { text: Text? }
  output {
    length: Int(-1000..1000); label: Text(1..5); enabled: Bool; state: State; note: Text?
    owner: principal.Id; document: Document.Id; amount: Decimal(5,2); date: Date; at: Time; email: Email
  }
  allow actor != null
  effect none
  deadline 1s
  implementation "inspect.output"
}
"#;

#[test]
fn generated_helpers_reject_outputs_outside_their_declared_contract() {
    let facts = load_str(SOURCE, Form::A).expect("operation output definition").execution;
    for wire in [IdWire::SafeNumber, IdWire::DecimalString] {
        let generated = spike_v5_sdk::contract_module_python(&facts, wire);
        assert!(generated.contains("\"text\": NotRequired[Optional[str]]"));
        assert!(generated.contains("\"note\": Optional[str]"));
        assert!(!generated.contains("\"note\": NotRequired"));
        let script = r#"
import sys
sys.path.insert(0, sys.argv[1])
from aip_sdk import AipError
scope = {}
exec(sys.stdin.read(), scope)
call = scope['call_inspect']
class Stub:
    def __init__(self, output): self.output = output
    def operation(self, name, values):
        assert name == 'inspect'
        return self.output
valid = dict(length=-1, label='가😀', enabled=True, state='READY', note=None,
             owner=1 if scope['ID_WIRE']=='safe-number-v13' else '1',
             document='12345678-1234-1234-1234-123456789abc', amount='-12.34',
             date='2024-02-29', at='2024-02-29T23:59:60Z', email='a@example.test')
assert call(Stub(valid), {}) == valid
bad_outputs = [[], None, {k:v for k,v in valid.items() if k!='note'}, dict(valid, extra='field')]
for key, value in [('length', True), ('length', 1001), ('length', -1001), ('length', 1.5),
                   ('label', ''), ('label', 'abcdef'), ('label', 'nul\0'), ('enabled', 1),
                   ('state', 'MISSING'), ('note', 1), ('owner', True), ('owner', -1),
                   ('owner', '01'), ('document', 'not-uuid'), ('amount', '1.234'),
                   ('amount', '1000.00'), ('amount', 1.0), ('date', '2026-02-29'),
                   ('at', '2024-02-29T24:00:00Z'), ('email', 'not-email')]:
    bad_outputs.append(dict(valid, **{key:value}))
for output in bad_outputs:
    try:
        call(Stub(output), {})
    except AipError as error:
        assert error.code == 'CLIENT.PROTOCOL_ERROR', error.code
    else:
        raise AssertionError('generated helper trusted output outside its declared contract: '+repr(output))
print('generated operation output checks passed')
"#;
        let mut child = Command::new("python3")
            .args(["-c", script, concat!(env!("CARGO_MANIFEST_DIR"), "/../../product/python")])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        child.stdin.take().unwrap().write_all(generated.as_bytes()).unwrap();
        let output = child.wait_with_output().unwrap();
        assert!(output.status.success(), "{wire:?}: {}", String::from_utf8_lossy(&output.stderr));
    }
}
