#!/usr/bin/env python3
import json
import os
import shutil
import subprocess
from pathlib import Path
from typing import Any, Optional


class DawnControlApiError(Exception):
    pass


DAWN_HOME = Path(
    str(os.environ.get('DAWN_HOME') or '').strip() or (Path.home() / '.dawn')
).expanduser()
RUNTIME_DIR = DAWN_HOME / 'runtime'
ACTIVE_RUNTIME_PATH = RUNTIME_DIR / 'active.json'
CONTROL_API_DISCOVERY_PATH = Path(
    str(os.environ.get('DAWN_CONTROL_API_DISCOVERY_PATH') or '').strip() or (RUNTIME_DIR / 'control-api.json')
)
SCRIPT_DIR = Path(__file__).resolve().parent
CALL_CONTROL_API_SCRIPT = SCRIPT_DIR / 'call_control_api.mjs'


def _read_json_file(path: Path, fallback: Any) -> Any:
    try:
        if not path.exists():
            return fallback
        raw = path.read_text(encoding='utf-8').strip()
        if not raw:
            return fallback
        return json.loads(raw)
    except Exception:
        return fallback


def _active_runtime_slot_dir() -> Optional[Path]:
    pointer = _read_json_file(ACTIVE_RUNTIME_PATH, {})
    if not isinstance(pointer, dict):
        return None

    slot_path = str(pointer.get('slot_path') or pointer.get('slotPath') or '').strip()
    if slot_path:
        candidate = Path(slot_path).expanduser()
        if candidate.exists():
            return candidate

    bundle_version = str(pointer.get('bundle_version') or pointer.get('bundleVersion') or '').strip()
    if bundle_version:
        candidate = RUNTIME_DIR / 'bundles' / bundle_version
        if candidate.exists():
            return candidate

    return None


def resolve_node_binary() -> str:
    explicit = str(os.environ.get('DAWN_NODE_BIN') or '').strip()
    if explicit and Path(explicit).exists():
        return explicit

    slot_dir = _active_runtime_slot_dir()
    if slot_dir:
        node_name = 'node.exe' if os.name == 'nt' else 'node'
        bundled = slot_dir / 'node' / node_name
        if bundled.exists():
            return str(bundled)

    node_on_path = shutil.which('node')
    if node_on_path:
        return node_on_path

    raise DawnControlApiError(
        'Could not find a Node runtime. Expected the active Dawn runtime slot to provide one.'
    )


def call_control_api(
    method: str,
    params: Optional[dict[str, Any]] = None,
    *,
    timeout_ms: int = 5000,
    discovery_path: Optional[Path] = None,
) -> Any:
    if not CALL_CONTROL_API_SCRIPT.exists():
        raise DawnControlApiError(f'Missing control API helper: {CALL_CONTROL_API_SCRIPT}')

    discovery = (discovery_path or CONTROL_API_DISCOVERY_PATH).expanduser()
    if not discovery.exists():
        raise DawnControlApiError(f'Missing Dawn control API discovery file: {discovery}')

    command = [
        resolve_node_binary(),
        str(CALL_CONTROL_API_SCRIPT),
        method,
        '--discovery',
        str(discovery),
        '--timeout-ms',
        str(timeout_ms),
    ]
    if params is not None:
        command.extend(['--params', json.dumps(params, ensure_ascii=False)])

    completed = subprocess.run(
        command,
        capture_output=True,
        text=True,
        env=os.environ.copy(),
    )
    if completed.returncode != 0:
        detail = (completed.stderr or completed.stdout or '').strip() or f'exit {completed.returncode}'
        raise DawnControlApiError(detail)

    stdout = (completed.stdout or '').strip()
    if not stdout:
        return {}
    try:
        return json.loads(stdout)
    except json.JSONDecodeError as exc:
        raise DawnControlApiError(f'Control API returned invalid JSON: {exc}') from exc
