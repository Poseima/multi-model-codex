#!/usr/bin/env python3
import json
import os
import sys
from pathlib import Path
from typing import Any, Optional

from control_api_client import DawnControlApiError, call_control_api

DAWN_HOME = Path(
    str(os.environ.get('DAWN_HOME') or '').strip() or (Path.home() / '.dawn')
).expanduser()
RUNTIME_DIR = DAWN_HOME / 'runtime'
ACTIVE_RUNTIME_PATH = RUNTIME_DIR / 'active.json'
CONTROL_API_DISCOVERY_PATH = Path(
    str(os.environ.get('DAWN_CONTROL_API_DISCOVERY_PATH') or '').strip() or (RUNTIME_DIR / 'control-api.json')
)
IM_COMPONENTS = {
    'whatsapp': 'dawnclaw',
    'feishu': 'dawn-feishu',
    'discord': 'dawn-discord',
}
CORE_CHECKS = {
    'dawn_home',
    'runtime_active_pointer',
    'runtime_manifest',
    'control_api_discovery',
}


def make_check(name: str, status: str, summary: str, details: Optional[dict[str, Any]] = None) -> dict[str, Any]:
    payload = {
        'name': name,
        'status': status,
        'summary': summary,
    }
    if details:
        payload['details'] = details
    return payload


def load_json_file(path: Path) -> tuple[Optional[Any], Optional[str]]:
    try:
        if not path.exists():
            return None, None
        raw = path.read_text(encoding='utf-8').strip()
        if not raw:
            return {}, None
        return json.loads(raw), None
    except Exception as exc:
        return None, str(exc)


def read_active_pointer() -> tuple[Optional[dict[str, Any]], dict[str, Any]]:
    payload, error = load_json_file(ACTIVE_RUNTIME_PATH)
    if error:
        return None, make_check(
            'runtime_active_pointer',
            'failed',
            'Failed to parse ~/.dawn/runtime/active.json',
            {'path': str(ACTIVE_RUNTIME_PATH), 'error': error},
        )
    if payload is None:
        return None, make_check(
            'runtime_active_pointer',
            'blocked',
            'Missing ~/.dawn/runtime/active.json',
            {'path': str(ACTIVE_RUNTIME_PATH)},
        )
    if not isinstance(payload, dict):
        return None, make_check(
            'runtime_active_pointer',
            'failed',
            'Runtime active pointer is not a JSON object',
            {'path': str(ACTIVE_RUNTIME_PATH)},
        )
    return payload, make_check(
        'runtime_active_pointer',
        'ready',
        'Found active Dawn runtime pointer',
        {
            'path': str(ACTIVE_RUNTIME_PATH),
            'bundleVersion': str(payload.get('bundle_version') or payload.get('bundleVersion') or '').strip(),
            'slotPath': str(payload.get('slot_path') or payload.get('slotPath') or '').strip(),
            'manifestPath': str(payload.get('manifest_path') or payload.get('manifestPath') or '').strip(),
        },
    )


def resolve_manifest_path(active_pointer: Optional[dict[str, Any]]) -> Optional[Path]:
    pointer = active_pointer or {}
    manifest_path = str(pointer.get('manifest_path') or pointer.get('manifestPath') or '').strip()
    if manifest_path:
        candidate = Path(manifest_path).expanduser()
        if candidate.exists():
            return candidate

    slot_path = str(pointer.get('slot_path') or pointer.get('slotPath') or '').strip()
    if slot_path:
        candidate = Path(slot_path).expanduser() / 'runtime-manifest.json'
        if candidate.exists():
            return candidate

    bundle_version = str(pointer.get('bundle_version') or pointer.get('bundleVersion') or '').strip()
    if bundle_version:
        candidate = RUNTIME_DIR / 'bundles' / bundle_version / 'runtime-manifest.json'
        if candidate.exists():
            return candidate

    return None


def load_manifest(active_pointer: Optional[dict[str, Any]]) -> tuple[Optional[dict[str, Any]], dict[str, Any]]:
    manifest_path = resolve_manifest_path(active_pointer)
    if not manifest_path:
        return None, make_check(
            'runtime_manifest',
            'blocked',
            'Could not locate the active runtime manifest',
            {'activePointerPath': str(ACTIVE_RUNTIME_PATH)},
        )

    payload, error = load_json_file(manifest_path)
    if error:
        return None, make_check(
            'runtime_manifest',
            'failed',
            'Failed to parse runtime manifest',
            {'path': str(manifest_path), 'error': error},
        )
    if not isinstance(payload, dict):
        return None, make_check(
            'runtime_manifest',
            'failed',
            'Runtime manifest is not a JSON object',
            {'path': str(manifest_path)},
        )
    components = payload.get('components') if isinstance(payload.get('components'), dict) else {}
    return payload, make_check(
        'runtime_manifest',
        'ready',
        'Loaded active runtime manifest',
        {
            'path': str(manifest_path),
            'runtimeVersion': str(payload.get('runtimeVersion') or payload.get('runtime_version') or '').strip(),
            'componentNames': sorted(components.keys()),
        },
    )


def load_control_api_discovery() -> tuple[Optional[dict[str, Any]], dict[str, Any]]:
    payload, error = load_json_file(CONTROL_API_DISCOVERY_PATH)
    if error:
        return None, make_check(
            'control_api_discovery',
            'failed',
            'Failed to parse Dawn control API discovery file',
            {'path': str(CONTROL_API_DISCOVERY_PATH), 'error': error},
        )
    if payload is None:
        return None, make_check(
            'control_api_discovery',
            'blocked',
            'Missing Dawn control API discovery file',
            {'path': str(CONTROL_API_DISCOVERY_PATH)},
        )
    if not isinstance(payload, dict):
        return None, make_check(
            'control_api_discovery',
            'failed',
            'Dawn control API discovery payload is not a JSON object',
            {'path': str(CONTROL_API_DISCOVERY_PATH)},
        )

    endpoint = str(
        payload.get('endpoint')
        or payload.get('socketPath')
        or payload.get('pipeName')
        or payload.get('path')
        or ''
    ).strip()
    transport = str(payload.get('transport') or '').strip()
    if not endpoint:
        return None, make_check(
            'control_api_discovery',
            'failed',
            'Dawn control API discovery file is missing an endpoint',
            {'path': str(CONTROL_API_DISCOVERY_PATH), 'transport': transport},
        )

    return payload, make_check(
        'control_api_discovery',
        'ready',
        'Loaded Dawn control API discovery',
        {
            'path': str(CONTROL_API_DISCOVERY_PATH),
            'transport': transport,
            'endpoint': endpoint,
        },
    )


def read_runtime_status() -> tuple[Optional[dict[str, Any]], dict[str, Any]]:
    try:
        result = call_control_api('runtime.get_status', timeout_ms=4000)
    except DawnControlApiError as exc:
        return None, make_check(
            'control_api_runtime_status',
            'blocked',
            'Failed to query runtime.get_status through the Dawn control API',
            {
                'path': str(CONTROL_API_DISCOVERY_PATH),
                'error': str(exc),
            },
        )

    if not isinstance(result, dict):
        return None, make_check(
            'control_api_runtime_status',
            'failed',
            'runtime.get_status returned a non-object result',
            {'path': str(CONTROL_API_DISCOVERY_PATH)},
        )

    components = result.get('components') if isinstance(result.get('components'), dict) else {}
    return result, make_check(
        'control_api_runtime_status',
        'ready',
        'runtime.get_status succeeded',
        {
            'path': str(CONTROL_API_DISCOVERY_PATH),
            'runtimeVersion': str(result.get('runtimeVersion') or '').strip(),
            'componentNames': sorted(components.keys()),
        },
    )


def normalize_component_entry(entry: Any) -> dict[str, Any]:
    if not isinstance(entry, dict):
        return {}
    if 'presence' in entry:
        presence = str(entry.get('presence') or '').strip()
        return {
            'present': presence == 'present',
            'reason': presence,
            'state': str(entry.get('state') or '').strip(),
            'version': str(entry.get('version') or '').strip(),
        }
    return {
        'present': bool(entry.get('present')),
        'reason': str(entry.get('reason') or '').strip(),
        'state': str(entry.get('state') or '').strip(),
        'version': str(entry.get('version') or '').strip(),
        'statusReason': str(entry.get('statusReason') or entry.get('status_reason') or '').strip(),
    }


def connector_check(im_type: str, components: dict[str, Any]) -> tuple[dict[str, Any], bool, bool]:
    component_name = IM_COMPONENTS[im_type]
    normalized = normalize_component_entry(components.get(component_name))
    ipc_root = DAWN_HOME / 'ipc' / im_type
    subdirs = [
        name
        for name in ('requests', 'responses', 'commands', 'command-results')
        if (ipc_root / name).exists()
    ]

    if normalized.get('present') is True:
        if ipc_root.exists():
            return make_check(
                f'ipc_root_{im_type}',
                'ready',
                f'{component_name} is present and the IPC root exists',
                {
                    'component': component_name,
                    'ipcRoot': str(ipc_root),
                    'state': normalized.get('state', ''),
                    'reason': normalized.get('reason', ''),
                    'subdirsPresent': subdirs,
                },
            ), True, False
        return make_check(
            f'ipc_root_{im_type}',
            'blocked',
            f'{component_name} is present but the IPC root is missing',
            {
                'component': component_name,
                'ipcRoot': str(ipc_root),
                'state': normalized.get('state', ''),
                'reason': normalized.get('reason', ''),
            },
        ), False, False

    if normalized.get('reason') == 'absent_by_design':
        return make_check(
            f'ipc_root_{im_type}',
            'ready',
            f'{component_name} is omitted by design in the active runtime',
            {
                'component': component_name,
                'ipcRoot': str(ipc_root),
                'state': normalized.get('state', ''),
                'reason': normalized.get('reason', ''),
            },
        ), False, True

    if ipc_root.exists():
        return make_check(
            f'ipc_root_{im_type}',
            'ready',
            f'IPC root exists for {im_type} without an explicit present component record',
            {
                'component': component_name,
                'ipcRoot': str(ipc_root),
                'subdirsPresent': subdirs,
                'reason': normalized.get('reason', ''),
            },
        ), True, False

    return make_check(
        f'ipc_root_{im_type}',
        'blocked',
        f'No active IPC root detected for {im_type}',
        {
            'component': component_name,
            'ipcRoot': str(ipc_root),
            'reason': normalized.get('reason', ''),
            'state': normalized.get('state', ''),
        },
    ), False, False


def overall_status(checks: list[dict[str, Any]], connector_ready_count: int) -> str:
    if any(check['status'] == 'failed' for check in checks):
        return 'failed'
    if any(check['status'] == 'blocked' for check in checks if check['name'] in CORE_CHECKS):
        return 'blocked'
    if connector_ready_count == 0:
        return 'blocked'
    return 'ready'


def main() -> int:
    checks: list[dict[str, Any]] = []

    if DAWN_HOME.exists():
        checks.append(make_check('dawn_home', 'ready', 'Found ~/.dawn runtime home', {'path': str(DAWN_HOME)}))
    else:
        checks.append(make_check('dawn_home', 'blocked', 'Missing ~/.dawn runtime home', {'path': str(DAWN_HOME)}))

    active_pointer, active_check = read_active_pointer()
    checks.append(active_check)

    manifest, manifest_check = load_manifest(active_pointer)
    checks.append(manifest_check)

    discovery, discovery_check = load_control_api_discovery()
    checks.append(discovery_check)

    runtime_status, runtime_status_check = read_runtime_status()
    checks.append(runtime_status_check)

    components: dict[str, Any] = {}
    if isinstance(runtime_status, dict):
        components = runtime_status.get('components') if isinstance(runtime_status.get('components'), dict) else {}
    if not components and isinstance(manifest, dict):
        components = manifest.get('components') if isinstance(manifest.get('components'), dict) else {}

    connector_ready_count = 0
    omitted_connectors: list[str] = []
    for im_type in IM_COMPONENTS:
        check, ready, omitted = connector_check(im_type, components)
        checks.append(check)
        if ready:
            connector_ready_count += 1
        if omitted:
            omitted_connectors.append(im_type)

    checks.append(make_check(
        'public_capability_handling',
        'ready',
        'Capability-gated public connector handling is enabled',
        {
            'omittedConnectors': omitted_connectors,
            'note': 'Connectors omitted by design are treated as capability-gated, not as runtime failures.',
        },
    ))

    status = overall_status(checks, connector_ready_count)
    blocked_or_failed = [
        f"{check['name']}: {check['summary']}"
        for check in checks
        if check['status'] in {'blocked', 'failed'}
    ]
    if status == 'ready':
        summary = f'Validation passed with {connector_ready_count} ready connector(s).'
    elif blocked_or_failed:
        summary = blocked_or_failed[0]
    else:
        summary = 'Validation failed.'
    payload = {
        'status': status,
        'summary': summary,
        'dawnHome': str(DAWN_HOME),
        'controlApiDiscoveryPath': str(CONTROL_API_DISCOVERY_PATH),
        'readyConnectorCount': connector_ready_count,
        'details': blocked_or_failed,
        'checks': checks,
    }
    print(json.dumps(payload, indent=2, ensure_ascii=False))
    if status == 'ready':
        return 0
    if status == 'blocked':
        return 2
    return 1


if __name__ == '__main__':
    sys.exit(main())
