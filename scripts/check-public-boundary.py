"""CI guard: public runtime manifests must not acquire private Hub dependencies."""
import json
import pathlib
import tomllib

root = pathlib.Path(__file__).resolve().parent.parent
cargo = tomllib.loads((root / 'Cargo.toml').read_text())
web = json.loads((root / 'web/package.json').read_text())
for manifest in [cargo, web]:
    serialized = json.dumps(manifest).lower()
    for forbidden in ['tolmap-hub', 'semon-hub', 'staging.tolmap.dev', 'app.tolmap.dev']:
        assert forbidden not in serialized, f'private runtime reference: {forbidden}'
# Public generated contracts are shipped without private account schemas.
for binding in (root / 'bindings').glob('*.ts'):
    data = binding.read_text().lower()
    assert 'tolmap-hub' not in data and 'semon-hub' not in data, binding
print('PASS public runtime manifests and generated bindings have no private-app dependency')
