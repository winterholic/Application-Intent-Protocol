import re
from pathlib import Path

root = Path(__file__).resolve().parents[1]
base = root / 'plan-docs'
errors = []

def require(condition, message):
    if not condition:
        errors.append(message)

def ids(text, pattern, expected):
    actual = re.findall(pattern, text, re.M)
    return len(actual) == len(expected) and set(actual) == set(expected)

def fences(text):
    count = 0
    for line in text.splitlines():
        if line.lstrip().startswith(chr(96) * 3):
            count += 1
    return count % 2 == 0

def markdown_errors(path, text):
    failures = []
    if not fences(text):
        failures.append('unclosed code fence')
    if text.count('<details>') != text.count('</details>'):
        failures.append('unclosed archive')
    for target in re.findall(r'\[[^\]\n]+\]\(([^)\n]+)\)', text):
        if target.startswith(('http://','https://','#','mailto:')):
            continue
        filename = target.split('#')[0]
        if not (path.parent/filename).exists():
            failures.append(f'broken link {target}')
    return failures

require(ids('| 01 | a |\n| 02 | b |', r'^\| (\d{2}) \|', ['01','02']), 'checker: valid table rejected')
require(not ids('| 01 | a |', r'^\| (\d{2}) \|', ['01','02']), 'checker: missing row accepted')
require(not ids('| 01 | a |\n| 01 | b |', r'^\| (\d{2}) \|', ['01','02']), 'checker: duplicate row accepted')
require(not fences(chr(96)*3+'text\nbad'), 'checker: unclosed fence accepted')
control_path = base / 'README.md'
require(markdown_errors(control_path, '[x](alignment/__missing_negative_control__.md)') ==
        ['broken link alignment/__missing_negative_control__.md'], 'checker: missing path accepted')
require(not markdown_errors(control_path, '[x](README.md)'), 'checker: valid path rejected')
require(markdown_errors(control_path, '<details>') == ['unclosed archive'], 'checker: unclosed archive accepted')

names = [
'A-founder-intent-matrix.md',
'B-decision-reclassification.md',
'C-syntax-proposal.md',
'D-development-experience.md',
'E-technical-risks.md',
]
texts = {}
for name in names:
    path = base / 'alignment' / name
    require(path.is_file(), f'missing deliverable: {name}')
    if path.is_file():
        texts[name[0]] = path.read_text()

source = base / 'sources/founder-integrated-directive-2026-10-03.md'
require(source.is_file(), 'missing founder source')
if source.is_file():
    original = source.read_text()
    require(ids(original, r'^# (\d+)\.', [str(i) for i in range(18)]), 'source: section 0..17 mismatch')
    require(ids(original, r'^## 원칙 (\d+)\.', [str(i) for i in range(1,8)]), 'source: seven principles mismatch')

b = texts.get('B', '')
require(ids(b, r'^\| (\d{2}) \|', [f'{i:02}' for i in range(1,26)]), 'B: 25 questions missing/duplicate')
require(ids(b, r'^\| (Q\d) \|', [f'Q{i}' for i in range(10)]), 'B: Q0..Q9 missing/duplicate')
require(ids(b, r'^\| (AG-\d{2}) \|', [f'AG-{i:02}' for i in range(1,15)]), 'B: 14 agendas missing/duplicate')
require(ids(b, r'^\| (DD-\d{2}) \|', [f'DD-{i:02}' for i in range(1,15)]), 'B: DD statuses missing/duplicate')
require(ids(texts.get('C',''), r'^\| (EQ-\d{2}) \|', [f'EQ-{i:02}' for i in range(1,12)]), 'C: EQ01..11 mismatch')
require(ids(texts.get('D',''), r'^\| (SC-\d{2}) \|', [f'SC-{i:02}' for i in range(1,11)]), 'D: 10 scenarios mismatch')
require(ids(texts.get('E',''), r'^\| (RK-\d{2}) \|', [f'RK-{i:02}' for i in range(1,11)]), 'E: 10 risk cases mismatch')

expected_states = dict(re.findall(r'^\| (DD-\d{2}) \| (잠정 합의|검증 필요) \|', b, re.M))
decisions = sorted((base/'decisions').glob('DD-*.md'))
require(len(decisions)==14, 'expected 14 decision files')
for path in decisions:
    text = path.read_text()
    key = path.name[:5]
    expected = expected_states.get(key)
    top = re.search(r'^> 결정 상태: \*\*(.*?)\*\*', text, re.M)
    g = re.search(r'^## G\. 결정 상태\s+\*\*(.*?)\.?\*\*', text, re.M)
    require(top is not None and top.group(1)==expected, f'{key}: top status mismatch')
    require(g is not None and g.group(1).rstrip('.')==expected, f'{key}: G status mismatch')
    require('## F.' in text, f'{key}: missing F section')
    require('B-decision-reclassification.md' in text, f'{key}: missing current basis')

topics = sorted((base/'topics').glob('T*.md'))
require(len(topics)==13, 'expected 13 topic files')
for path in topics:
    require('B-decision-reclassification.md' in path.read_text(), f'{path.name}: missing current basis')

paths = (
[base/'alignment'/name for name in names] + decisions + topics +
[base/name for name in ['README.md','00-rules.md','01-purpose.md','02-principles.md',
'03-glossary.md','04-current-state.md','05-agenda-status.md','90-open-questions.md','91-roadmap.md','STATUS.md']] +
[base/'decisions/README.md', base/'reviews/INTEGRATED-codex-luna-r1.md',
base/'alignment/V0-validation-results.md',
root/'README.md', root/'docs/PRINCIPLES.md', root/'docs/DECISIONS.md', source]
)
# Include new experiments and independent reviews as they are added.
paths = sorted(set(paths) | set((base/'alignment').glob('*.md')) | set((base/'reviews').glob('*.md')))
for path in paths:
    require(path.is_file(), f'missing checked file: {path}')
    if not path.is_file():
        continue
    text = path.read_text()
    for failure in markdown_errors(path, text):
        errors.append(f'{path.relative_to(root)}: {failure}')
    require(not re.search(r'[未読候不]수행|候보|読기|不변|非同期', text), f'{path.name}: prose typo')

print('negative controls: missing row / duplicate row / unclosed fence / missing path rejected')
print(f'deliverables 5; questions 25; legacy 10; agendas 14; decisions {len(decisions)}; topics {len(topics)}')
print(f'markdown files checked: {len(paths)}; errors: {len(errors)}')
for error in errors:
    print('ERROR:', error)
raise SystemExit(bool(errors))
