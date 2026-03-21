#!/usr/bin/env python3
import argparse
import base64
import hashlib
import hmac
import json
import os
import re
import select
import subprocess
import sys
import time
import uuid
from datetime import datetime, timezone
from pathlib import Path
from types import SimpleNamespace
from typing import Any, Optional

from control_api_client import DawnControlApiError, call_control_api

DAWN_HOME = Path(
    str(os.environ.get('DAWN_HOME') or '').strip() or (Path.home() / '.dawn')
).expanduser()
RUNTIME_DIR = DAWN_HOME / 'runtime'
ACTIVE_RUNTIME_PATH = RUNTIME_DIR / 'active.json'
CONNECTORS_DIR = DAWN_HOME / 'connectors'
FEISHU_CONNECTOR_CONFIG_PATH = CONNECTORS_DIR / 'feishu' / 'config.json'
AVATARS_DIR = DAWN_HOME / 'avatars'
CONTEXT_BINDINGS_DIR = DAWN_HOME / 'ipc' / 'context-bindings'
CONTROL_API_DISCOVERY_PATH = Path(
    str(os.environ.get('DAWN_CONTROL_API_DISCOVERY_PATH') or '').strip() or (RUNTIME_DIR / 'control-api.json')
)
IM_COMPONENTS = {
    'whatsapp': 'dawnclaw',
    'feishu': 'dawn-feishu',
    'discord': 'dawn-discord',
}
IM_TYPES = tuple(IM_COMPONENTS.keys())
SERVICE_COMPONENTS = {
    'dawnclaw': 'dawnclaw',
    'dawn-feishu': 'dawn-feishu',
    'dawn-discord': 'dawn-discord',
}
SERVICE_NAMES = tuple(SERVICE_COMPONENTS.keys())
SERVICE_DEFS: dict[str, dict[str, str]] = {
    'dawnclaw':     {'pid_file': 'dawnclaw.pid',     'heartbeat': 'dawnclaw.json',     'cwd_marker': 'dawnclaw'},
    'dawn-feishu':  {'pid_file': 'dawn-feishu.pid',  'heartbeat': 'dawn-feishu.json',  'cwd_marker': 'dawn-feishu'},
    'dawn-discord': {'pid_file': 'dawn-discord.pid', 'heartbeat': 'dawn-discord.json', 'cwd_marker': 'dawn-discord'},
}
BACKGROUND_DISPATCH_DIR = DAWN_HOME / 'ipc' / 'im-dispatch'
BACKGROUND_REQUESTS_DIR = BACKGROUND_DISPATCH_DIR / 'requests'
BACKGROUND_JOBS_DIR = BACKGROUND_DISPATCH_DIR / 'jobs'
CAPABILITIES_CACHE_PATH = DAWN_HOME / 'cache' / 'codex-capabilities.json'


class DawnImError(Exception):
    pass


def _now_iso() -> str:
    return time.strftime('%Y-%m-%dT%H:%M:%SZ', time.gmtime())


def _read_json(path: Path, fallback: Any) -> Any:
    try:
        if not path.exists():
            return fallback
        raw = path.read_text(encoding='utf-8').strip()
        if not raw:
            return fallback
        return json.loads(raw)
    except Exception:
        return fallback


def _write_json_atomic(path: Path, data: Any) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    tmp = path.with_name(f"{path.name}.tmp-{int(time.time() * 1000)}-{uuid.uuid4().hex[:6]}")
    tmp.write_text(json.dumps(data, indent=2, ensure_ascii=False), encoding='utf-8')
    os.replace(tmp, path)


def _infer_im_type_from_folder_name(group_folder: str) -> str:
    folder = str(group_folder or '').strip().lower()
    if folder.startswith('discord-'):
        return 'discord'
    if folder.startswith('feishu-'):
        return 'feishu'
    if folder.startswith('whatsapp-'):
        return 'whatsapp'
    return ''


def _read_active_runtime_pointer() -> dict[str, Any]:
    payload = _read_json(ACTIVE_RUNTIME_PATH, {})
    return payload if isinstance(payload, dict) else {}


def _active_runtime_slot_dir() -> Optional[Path]:
    pointer = _read_active_runtime_pointer()
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


def _active_runtime_manifest_path() -> Optional[Path]:
    pointer = _read_active_runtime_pointer()
    manifest_path = str(pointer.get('manifest_path') or pointer.get('manifestPath') or '').strip()
    if manifest_path:
        candidate = Path(manifest_path).expanduser()
        if candidate.exists():
            return candidate
    slot_dir = _active_runtime_slot_dir()
    if slot_dir:
        candidate = slot_dir / 'runtime-manifest.json'
        if candidate.exists():
            return candidate
    return None


def _load_runtime_manifest() -> dict[str, Any]:
    manifest_path = _active_runtime_manifest_path()
    if not manifest_path:
        return {}
    payload = _read_json(manifest_path, {})
    return payload if isinstance(payload, dict) else {}


def _runtime_component_presence(component_name: str) -> str:
    manifest = _load_runtime_manifest()
    components = manifest.get('components') if isinstance(manifest.get('components'), dict) else {}
    entry = components.get(component_name)
    if not isinstance(entry, dict):
        return ''
    return str(entry.get('presence') or '').strip()


def _detected_im_types() -> list[str]:
    detected: set[str] = set()
    for im_type, component_name in IM_COMPONENTS.items():
        if _runtime_component_presence(component_name) == 'present':
            detected.add(im_type)
    for im_type in IM_TYPES:
        if (DAWN_HOME / 'ipc' / im_type).exists():
            detected.add(im_type)
    return [im_type for im_type in IM_TYPES if im_type in detected]


def _default_im_type() -> str:
    detected = _detected_im_types()
    return detected[0] if len(detected) == 1 else ''


def _detected_service_names() -> list[str]:
    detected: set[str] = set()
    for service_name, component_name in SERVICE_COMPONENTS.items():
        if _runtime_component_presence(component_name) == 'present':
            detected.add(service_name)
        sdef = SERVICE_DEFS.get(service_name)
        if not sdef:
            continue
        pid_path = DAWN_HOME / sdef['pid_file']
        hb_path = DAWN_HOME / 'heartbeat' / sdef['heartbeat']
        if pid_path.exists() or hb_path.exists():
            detected.add(service_name)
    return [service_name for service_name in SERVICE_NAMES if service_name in detected]


def _ipc_dir(im_type: str, subdir: str) -> Path:
    return DAWN_HOME / 'ipc' / im_type / subdir


def _write_im_command(im_type: str, payload: dict[str, Any]) -> None:
    commands_dir = _ipc_dir(im_type, 'commands')
    commands_dir.mkdir(parents=True, exist_ok=True)
    fp = commands_dir / f"{int(time.time() * 1000)}-{uuid.uuid4().hex[:8]}.json"
    tmp = Path(str(fp) + '.tmp')
    tmp.write_text(json.dumps(payload, indent=2, ensure_ascii=False), encoding='utf-8')
    os.replace(tmp, fp)


def _poll_im_command_result(im_type: str, command_id: str, timeout_ms: int = 10000) -> dict[str, Any]:
    results_dir = _ipc_dir(im_type, 'command-results')
    start = time.time()
    timeout = timeout_ms / 1000
    while time.time() - start < timeout:
        if results_dir.exists():
            for item in results_dir.iterdir():
                if item.suffix != '.json':
                    continue
                try:
                    payload = json.loads(item.read_text(encoding='utf-8'))
                except Exception:
                    continue
                if payload.get('commandId') == command_id:
                    try:
                        item.unlink(missing_ok=True)
                    except Exception:
                        pass
                    return payload
        time.sleep(0.1)
    return {'success': False, 'commandId': command_id, 'error': 'Timed out waiting for IM service response'}


def _write_ipc_request(im_type: str, payload: dict[str, Any]) -> None:
    requests_dir = _ipc_dir(im_type, 'requests')
    requests_dir.mkdir(parents=True, exist_ok=True)
    fp = requests_dir / f"{int(time.time() * 1000)}-{uuid.uuid4().hex[:8]}.json"
    tmp = Path(str(fp) + '.tmp')
    tmp.write_text(json.dumps(payload, indent=2, ensure_ascii=False), encoding='utf-8')
    os.replace(tmp, fp)


def _background_job_path(job_id: str) -> Path:
    return BACKGROUND_JOBS_DIR / f'{job_id}.json'


def _read_background_job(job_id: str) -> Optional[dict[str, Any]]:
    payload = _read_json(_background_job_path(job_id), None)
    return payload if isinstance(payload, dict) else None


def _poll_background_job(job_id: str, timeout_ms: int = 2000) -> Optional[dict[str, Any]]:
    timeout = max(timeout_ms, 0) / 1000
    start = time.time()
    while time.time() - start < timeout:
        job = _read_background_job(job_id)
        if job:
            return job
        time.sleep(0.1)
    return _read_background_job(job_id)


def _poll_command_response(im_type: str, request_id: str, timeout_ms: int = 15000) -> dict[str, Any]:
    responses_dir = _ipc_dir(im_type, 'responses')
    start = time.time()
    timeout = timeout_ms / 1000
    while time.time() - start < timeout:
        if responses_dir.exists():
            for item in responses_dir.iterdir():
                if item.suffix != '.json':
                    continue
                try:
                    payload = json.loads(item.read_text(encoding='utf-8'))
                except Exception:
                    continue
                if payload.get('id') == request_id and payload.get('type') == 'command_response':
                    try:
                        item.unlink(missing_ok=True)
                    except Exception:
                        pass
                    return payload
        time.sleep(0.5)
    return {'ok': False, 'message': 'Timed out waiting for bridge response'}


def _list_avatar_ids() -> list[str]:
    if not AVATARS_DIR.exists():
        return []
    avatars: list[str] = []
    for p in AVATARS_DIR.iterdir():
        if (p / 'config.json').exists():
            avatars.append(p.name)
    return avatars


def _avatar_config_path(avatar_id: str) -> Path:
    return AVATARS_DIR / avatar_id / 'config.json'


def _avatar_state_path(avatar_id: str) -> Path:
    return AVATARS_DIR / avatar_id / 'state.json'


def _resolve_im_type_for_group(group_folder: str) -> str:
    if not group_folder:
        return _default_im_type()
    for avatar_id in _list_avatar_ids():
        cfg = _read_json(_avatar_config_path(avatar_id), {})
        gc = ((cfg.get('groupConfigs') or {}).get(group_folder) or {})
        im = str(gc.get('imType') or '').strip()
        if im:
            return im
    inferred = _infer_im_type_from_folder_name(group_folder)
    if inferred:
        return inferred
    return _default_im_type()


def _find_avatar_for_group(group_folder: str) -> str:
    for avatar_id in _list_avatar_ids():
        cfg = _read_json(_avatar_config_path(avatar_id), {})
        groups = cfg.get('groups') or []
        if group_folder in groups:
            return avatar_id
    return ''


def _find_folder_by_display_name(name: str, im_type: Optional[str] = None) -> Optional[str]:
    for avatar_id in _list_avatar_ids():
        cfg = _read_json(_avatar_config_path(avatar_id), {})
        for folder, meta in (cfg.get('groupConfigs') or {}).items():
            if (meta or {}).get('displayName') == name and (not im_type or (meta or {}).get('imType') == im_type):
                return folder
    return None


def _find_avatar_feishu_source_group(avatar_id: str) -> str:
    if not avatar_id:
        return ''
    cfg = _read_json(_avatar_config_path(avatar_id), {})
    groups = cfg.get('groups') if isinstance(cfg.get('groups'), list) else []
    group_configs = cfg.get('groupConfigs') if isinstance(cfg.get('groupConfigs'), dict) else {}

    for raw in groups:
        folder = str(raw).strip()
        if not folder or folder == 'main':
            continue
        meta = group_configs.get(folder) if isinstance(group_configs, dict) else {}
        if (meta or {}).get('imType') == 'feishu':
            return folder

    for folder, meta in group_configs.items():
        f = str(folder).strip()
        if f and f != 'main' and (meta or {}).get('imType') == 'feishu':
            return f
    return ''


def _parse_iso8601_utc(value: str) -> float:
    text = str(value or '').strip()
    if not text:
        return 0.0
    try:
        return datetime.fromisoformat(text.replace('Z', '+00:00')).astimezone(timezone.utc).timestamp()
    except Exception:
        return 0.0


def _base64url_decode_text(value: str) -> str:
    normalized = str(value or '').replace('-', '+').replace('_', '/')
    padding = '=' * ((4 - (len(normalized) % 4)) % 4)
    return base64.b64decode(f'{normalized}{padding}').decode('utf-8')


def _binding_value(binding: dict[str, Any], *keys: str) -> str:
    for key in keys:
        value = str(binding.get(key) or '').strip()
        if value:
            return value
    return ''


def _resolve_context_secret_path(group_folder: str, im_type: str = '') -> Optional[Path]:
    resolved_im_type = str(im_type or '').strip()
    if not resolved_im_type and group_folder:
        for avatar_id in _list_avatar_ids():
            cfg = _read_json(_avatar_config_path(avatar_id), {})
            gc = ((cfg.get('groupConfigs') or {}).get(group_folder) or {})
            candidate = str(gc.get('imType') or '').strip()
            if candidate:
                resolved_im_type = candidate
                break
    if not resolved_im_type:
        resolved_im_type = _infer_im_type_from_folder_name(group_folder) or _default_im_type()
    if not resolved_im_type:
        return None
    return DAWN_HOME / 'ipc' / resolved_im_type / 'context-secret'


def _resolve_context_tokens_dir(group_folder: str, im_type: str = '') -> Optional[Path]:
    resolved_im_type = str(im_type or '').strip()
    if not resolved_im_type and group_folder:
        for avatar_id in _list_avatar_ids():
            cfg = _read_json(_avatar_config_path(avatar_id), {})
            gc = ((cfg.get('groupConfigs') or {}).get(group_folder) or {})
            candidate = str(gc.get('imType') or '').strip()
            if candidate:
                resolved_im_type = candidate
                break
    if not resolved_im_type:
        resolved_im_type = _infer_im_type_from_folder_name(group_folder) or _default_im_type()
    if not resolved_im_type:
        return None
    return DAWN_HOME / 'ipc' / resolved_im_type / 'context-tokens'


def _find_context_binding_file(thread_id: str) -> Optional[Path]:
    filename = f'{thread_id}.json'
    primary = CONTEXT_BINDINGS_DIR / filename
    if primary.exists():
        return primary
    for im_type in IM_TYPES:
        legacy = DAWN_HOME / 'ipc' / im_type / 'context-bindings' / filename
        if legacy.exists():
            return legacy
    return None


def _verify_context_token(token: str) -> dict[str, Any]:
    raw = str(token or '').strip()
    if '.' not in raw:
        raise DawnImError('Invalid context token format')

    payload_b64, signature_hex = raw.split('.', 1)
    try:
        payload = json.loads(_base64url_decode_text(payload_b64))
    except Exception as err:
        raise DawnImError(f'Invalid context token payload: {err}') from err
    if not isinstance(payload, dict):
        raise DawnImError('Invalid context token payload')

    group_folder = str(payload.get('group_folder') or '').strip()
    token_im_type = str(payload.get('im_type') or '').strip()

    secret_path = _resolve_context_secret_path(group_folder, token_im_type)
    if not secret_path:
        raise DawnImError('Could not resolve the IM type for the current Dawn context token')
    try:
        secret = secret_path.read_text(encoding='utf-8').strip()
    except Exception as err:
        raise DawnImError(f'Missing context secret: {secret_path}') from err
    if not secret:
        raise DawnImError('Missing context secret')

    expected_sig = hmac.new(secret.encode('utf-8'), payload_b64.encode('utf-8'), hashlib.sha256).hexdigest()
    if not hmac.compare_digest(expected_sig, signature_hex):
        raise DawnImError('Invalid context token signature')

    expires_at = str(payload.get('expires_at') or '').strip()
    expires_at_secs = _parse_iso8601_utc(expires_at)
    if not expires_at_secs or expires_at_secs < time.time():
        raise DawnImError('Context token expired')

    token_id = str(payload.get('token_id') or '').strip()
    if not token_id:
        raise DawnImError('Context token missing token_id')

    tokens_dir = _resolve_context_tokens_dir(group_folder, token_im_type)
    if not tokens_dir:
        raise DawnImError('Could not resolve the IM token directory for the current Dawn context token')
    token_file = tokens_dir / f'{token_id}.json'
    if not token_file.exists():
        raise DawnImError('Context token unknown')

    token_meta = _read_json(token_file, {})
    if not isinstance(token_meta, dict):
        raise DawnImError('Context token metadata unreadable')
    if _binding_value(token_meta, 'token_id', 'tokenId') != token_id or _binding_value(token_meta, 'signature') != signature_hex:
        raise DawnImError('Context token metadata mismatch')

    meta_checks = {
        'group_folder': group_folder,
        'avatar_id': str(payload.get('avatar_id') or '').strip(),
        'chat_jid': str(payload.get('chat_jid') or '').strip(),
        'thread_id': str(payload.get('thread_id') or '').strip(),
        'expires_at': expires_at,
    }
    for key, expected in meta_checks.items():
        actual = _binding_value(token_meta, key)
        if actual and expected and actual != expected:
            raise DawnImError(f'Context token metadata mismatch for {key}')

    return {
        'tokenId': token_id,
        'groupFolder': group_folder,
        'avatarId': str(payload.get('avatar_id') or '').strip(),
        'chatJid': str(payload.get('chat_jid') or '').strip(),
        'threadId': str(payload.get('thread_id') or '').strip(),
        'isMainGroup': bool(payload.get('is_main_group')),
        'imType': token_im_type or _resolve_im_type_for_group(group_folder),
        'expiresAt': expires_at,
        'contextToken': raw,
    }


def _resolve_current_context(thread_id: str = '') -> dict[str, Any]:
    resolved_thread_id = str(thread_id or os.environ.get('CODEX_THREAD_ID') or '').strip()
    if not resolved_thread_id:
        raise DawnImError('Missing current Dawn context: CODEX_THREAD_ID is not set')

    binding_path = _find_context_binding_file(resolved_thread_id)
    if not binding_path:
        raise DawnImError(f'Missing current Dawn context binding for thread {resolved_thread_id}')

    binding = _read_json(binding_path, {})
    if not isinstance(binding, dict):
        raise DawnImError(f'Invalid current Dawn context binding: {binding_path}')

    binding_thread_id = _binding_value(binding, 'threadId', 'thread_id')
    if binding_thread_id and binding_thread_id != resolved_thread_id:
        raise DawnImError('Current Dawn context binding thread mismatch')

    token = _binding_value(binding, 'contextToken', 'context_token')
    if not token:
        raise DawnImError('Missing current Dawn context token')

    token_payload = _verify_context_token(token)
    if token_payload['threadId'] != resolved_thread_id:
        raise DawnImError('Current Dawn context token thread mismatch')

    binding_checks = {
        'groupFolder': _binding_value(binding, 'groupFolder', 'group_folder'),
        'avatarId': _binding_value(binding, 'avatarId', 'avatar_id'),
        'chatJid': _binding_value(binding, 'chatJid', 'chat_jid'),
        'imType': _binding_value(binding, 'imType', 'im_type'),
    }
    for key, expected in binding_checks.items():
        actual = str(token_payload.get(key) or '').strip()
        if expected and actual and expected != actual:
            raise DawnImError(f'Current Dawn context binding mismatch for {key}')

    return {
        'threadId': resolved_thread_id,
        'groupFolder': str(token_payload.get('groupFolder') or '').strip(),
        'avatarId': str(token_payload.get('avatarId') or '').strip(),
        'chatJid': str(token_payload.get('chatJid') or '').strip(),
        'imType': str(token_payload.get('imType') or _binding_value(binding, 'imType', 'im_type') or '').strip(),
        'contextToken': token,
        'expiresAt': str(token_payload.get('expiresAt') or '').strip(),
        'tokenId': str(token_payload.get('tokenId') or '').strip(),
        'isMainGroup': bool(token_payload.get('isMainGroup')),
    }


def _resolve_with_current(flag_name: str, explicit: str, current: str, force_current: bool) -> str:
    explicit_value = str(explicit or '').strip()
    current_value = str(current or '').strip()
    if force_current:
        if not current_value:
            raise DawnImError(f'Current Dawn context missing {flag_name}')
        if explicit_value and explicit_value != current_value:
            raise DawnImError(f'--{flag_name} conflicts with current context ({current_value})')
        return current_value
    return explicit_value or current_value


def _try_resolve_current_context(thread_id: str = '') -> Optional[dict[str, Any]]:
    candidate_thread_id = str(thread_id or os.environ.get('CODEX_THREAD_ID') or '').strip()
    if not candidate_thread_id:
        return None
    try:
        return _resolve_current_context(candidate_thread_id)
    except DawnImError:
        return None


def _coerce_bool(value: Any, default: bool = False) -> bool:
    if value is None:
        return default
    if isinstance(value, bool):
        return value
    text = str(value).strip().lower()
    if not text:
        return default
    if text in {'1', 'true', 'yes', 'on'}:
        return True
    if text in {'0', 'false', 'no', 'off'}:
        return False
    return default


def _prepare_feishu_create_params(
    avatar_id: str,
    members: list[str],
    source_group: str,
    timeout_ms: int,
) -> tuple[dict[str, Any], str, bool]:
    params: dict[str, Any] = {}
    auto_selected_source_group = False

    if members:
        params['members'] = members
    source_group_resolved = str(source_group or '').strip()
    if source_group_resolved:
        params['sourceGroup'] = source_group_resolved

    if not members:
        if not source_group_resolved:
            source_group_resolved = _find_avatar_feishu_source_group(avatar_id)
            if source_group_resolved:
                params['sourceGroup'] = source_group_resolved
                auto_selected_source_group = True
        if not source_group_resolved:
            raise DawnImError(
                'For feishu create_channel, provide members or source_group. '
                'No existing feishu group found for avatar to copy members from.'
            )
        channels = _list_channels_once('feishu', timeout_ms=timeout_ms)
        if source_group_resolved not in channels:
            raise DawnImError(f"source_group not found in feishu registered channels: {source_group_resolved}")

    return params, source_group_resolved, auto_selected_source_group


def _generate_unique_folder(im_type: str, name: str) -> str:
    sanitized = re.sub(r'[^a-z0-9]+', '-', name.strip().lower())
    sanitized = re.sub(r'^-+|-+$', '', sanitized)[:40]
    if not sanitized:
        sanitized = 'channel'
    return f"{im_type}-{sanitized}-{uuid.uuid4().hex[:6]}"


def _add_group_binding(
    avatar_id: str,
    group: str,
    im_type: str,
    display_name: Optional[str] = None,
    inherit_group: Optional[str] = None,
) -> dict[str, Any]:
    cfg_path = _avatar_config_path(avatar_id)
    if not cfg_path.exists():
        raise DawnImError(f"Avatar '{avatar_id}' config not found")
    cfg = _read_json(cfg_path, {})
    groups = cfg.get('groups') if isinstance(cfg.get('groups'), list) else []
    existing = {str(g).strip() for g in groups if str(g).strip()}
    existing.add(group)
    cfg['groups'] = sorted(existing)

    group_configs = cfg.get('groupConfigs') if isinstance(cfg.get('groupConfigs'), dict) else {}
    current_entry = group_configs.get(group)
    next_entry = dict(current_entry) if isinstance(current_entry, dict) else {}
    next_entry['imType'] = im_type
    next_entry['displayName'] = display_name or group

    if inherit_group and 'codex' not in next_entry:
        source_entry = group_configs.get(inherit_group)
        source_codex = source_entry.get('codex') if isinstance(source_entry, dict) else None
        if isinstance(source_codex, dict):
            next_entry['codex'] = dict(source_codex)

    group_configs[group] = next_entry
    cfg['groupConfigs'] = group_configs
    _write_json_atomic(cfg_path, cfg)
    return {'ok': True, 'changedPaths': [str(cfg_path)]}


def _remove_group_bindings(folder: str, avatar_id: Optional[str] = None) -> dict[str, Any]:
    candidates = [avatar_id] if avatar_id else _list_avatar_ids()
    unbound: list[str] = []
    for aid in candidates:
        if not aid:
            continue
        cfg_path = _avatar_config_path(aid)
        if not cfg_path.exists():
            continue
        cfg = _read_json(cfg_path, {})
        current_groups = cfg.get('groups') if isinstance(cfg.get('groups'), list) else []
        next_groups = [g for g in current_groups if str(g).strip() != folder]
        group_configs = cfg.get('groupConfigs') if isinstance(cfg.get('groupConfigs'), dict) else {}
        had = len(next_groups) != len(current_groups) or folder in group_configs
        if not had:
            continue
        cfg['groups'] = next_groups
        if folder in group_configs:
            del group_configs[folder]
        cfg['groupConfigs'] = group_configs
        _write_json_atomic(cfg_path, cfg)
        unbound.append(aid)
    return {'changed': len(unbound), 'unboundAvatars': unbound}


def _remove_group_thread_state(folder: str) -> None:
    for aid in _list_avatar_ids():
        sp = _avatar_state_path(aid)
        if not sp.exists():
            continue
        st = _read_json(sp, {})
        threads = st.get('threads') if isinstance(st.get('threads'), dict) else {}
        if folder in threads:
            del threads[folder]
            st['threads'] = threads
            _write_json_atomic(sp, st)


def _normalize_action(action: str) -> str:
    mapping = {
        'channel_info': 'channel-info',
        'channel_edit': 'channel-edit',
        'member_info': 'member-info',
        'add_participant': 'addParticipant',
        'remove_participant': 'removeParticipant',
        'leave_group': 'leaveGroup',
        'bot_add': 'bot-add',
        'bot_remove': 'bot-remove',
        'bot_invite_url': 'bot-invite-url',
        'bot_list': 'bot-list',
        'app_add': 'app-add',
        'app_remove': 'app-remove',
        'app_list': 'app-list',
        'thread_create': 'thread-create',
        'thread_list': 'thread-list',
        'thread_reply': 'thread-reply',
        'role_info': 'role-info',
        'role_add': 'role-add',
        'role_remove': 'role-remove',
        'set_presence': 'set-presence',
        'emoji_list': 'emoji-list',
        'voice_status': 'voice-status',
    }
    return mapping.get(action, action)


def _exec_im_command(
    im_type: str,
    action: str,
    channel: str = '',
    timeout_ms: int = 15000,
    params: Optional[dict[str, Any]] = None,
    legacy_fields: Optional[dict[str, Any]] = None,
) -> dict[str, Any]:
    command_id = str(uuid.uuid4())
    payload: dict[str, Any] = {
        'id': command_id,
        'type': 'im_command',
        'action': action,
        'imType': im_type,
        'channel': channel,
        'timestamp': _now_iso(),
    }
    if params is not None:
        payload['params'] = params
    if legacy_fields:
        payload.update({k: v for k, v in legacy_fields.items() if v is not None})
    _write_im_command(im_type, payload)
    result = _poll_im_command_result(im_type, command_id, timeout_ms=timeout_ms)
    if 'success' not in result:
        raise DawnImError('Missing command result')
    return result


def _list_channels_once(im_type: str, timeout_ms: int = 8000) -> dict[str, Any]:
    result = _exec_im_command(im_type, 'list_channels', timeout_ms=timeout_ms)
    if not result.get('success'):
        raise DawnImError(f"list_channels failed: {result.get('error', 'unknown error')}")
    channels = ((result.get('data') or {}).get('channels') or {})
    if not isinstance(channels, dict):
        raise DawnImError('list_channels result missing channels map')
    return channels


def _require_im_type(im_type: str) -> str:
    if im_type not in IM_TYPES:
        raise DawnImError(f"Invalid im_type '{im_type}'. Use one of: {', '.join(IM_TYPES)}")
    return im_type


def _background_item_field(item: dict[str, Any], *keys: str) -> Any:
    for key in keys:
        if key in item:
            return item[key]
    return None


def _normalize_background_item(item: dict[str, Any], index: int) -> dict[str, Any]:
    if not isinstance(item, dict):
        raise DawnImError(f'background item #{index + 1} must be a JSON object')

    item_id = str(_background_item_field(item, 'id') or f'item-{index + 1}').strip()
    group_folder = str(_background_item_field(item, 'groupFolder', 'group_folder', 'group') or '').strip()
    avatar_id = str(_background_item_field(item, 'avatarId', 'avatar_id') or '').strip()
    prompt = str(_background_item_field(item, 'prompt', 'text') or '').strip()
    title = str(_background_item_field(item, 'title', 'name') or '').strip()
    presentation_mode = str(_background_item_field(
        item,
        'presentationMode',
        'presentation_mode',
    ) or '').strip()
    visible_text = str(_background_item_field(
        item,
        'visibleText',
        'visible_text',
    ) or '').strip()
    metadata = _background_item_field(item, 'metadata')

    if metadata is None:
        metadata = {}

    return {
        'id': item_id,
        'groupFolder': group_folder,
        'avatarId': avatar_id,
        'prompt': prompt,
        'title': title,
        'presentationMode': presentation_mode,
        'visibleText': visible_text,
        'metadata': metadata,
    }


def _summarize_background_job(job: dict[str, Any]) -> dict[str, Any]:
    items = job.get('items') if isinstance(job.get('items'), list) else []
    counts: dict[str, int] = {}
    for item in items:
        status = 'unknown'
        if isinstance(item, dict):
            raw_status = str(item.get('status') or '').strip()
            status = raw_status or 'unknown'
        counts[status] = counts.get(status, 0) + 1

    return {
        'jobId': str(job.get('jobId') or '').strip(),
        'itemCount': len(items),
        'counts': counts,
        'terminal': all(
            str((item or {}).get('status') or '').strip() in {
                'done',
                'failed_terminal',
                'delivery_failed',
                'canceled',
            }
            for item in items
            if isinstance(item, dict)
        ) if items else False,
        'updatedAt': str(job.get('updatedAt') or '').strip(),
    }


# ---------------------------------------------------------------------------
# Service restart helpers
# ---------------------------------------------------------------------------

def cmd_service_restart(args: argparse.Namespace) -> dict[str, Any]:
    if args.service == 'all':
        target_services = _detected_service_names()
        if not target_services:
            raise DawnImError(
                'No supervised Dawn IM services detected in the active runtime. '
                'Run scripts/validate.py or pass --service explicitly.'
            )
    else:
        presence = _runtime_component_presence(SERVICE_COMPONENTS.get(args.service, ''))
        if presence and presence != 'present':
            raise DawnImError(
                f"Service '{args.service}' is '{presence}' in the active runtime and cannot be restarted."
            )
        target_services = [args.service]
    if args.include_settings:
        raise DawnImError(
            'Restarting Dawn Settings itself is not supported from the built-in dawn-im-management skill. '
            'Restart the desktop app manually if needed.'
        )

    try:
        response = call_control_api(
            'runtime.restart_services',
            {
                'services': target_services,
                'includeSettings': False,
                'timeoutMs': int(args.timeout_ms),
            },
            timeout_ms=max(int(args.timeout_ms), 5000),
        )
    except DawnControlApiError as exc:
        raise DawnImError(f'Control API restart failed: {exc}') from exc

    if not isinstance(response, dict):
        raise DawnImError('runtime.restart_services returned an invalid payload')

    results = response.get('results') if isinstance(response.get('results'), list) else []
    warnings = response.get('warnings') if isinstance(response.get('warnings'), list) else []
    normalized_results: dict[str, Any] = {}
    restarted_services: list[str] = []
    failed_services: list[str] = []

    for item in results:
        if not isinstance(item, dict):
            continue
        service_name = str(item.get('service') or '').strip()
        if not service_name:
            continue
        status = str(item.get('status') or '').strip() or 'unknown'
        normalized_results[service_name] = item
        if status in {'restarted', 'running'}:
            restarted_services.append(service_name)
        elif status in {'omitted', 'blocked'} and _runtime_component_presence(SERVICE_COMPONENTS.get(service_name, '')) != 'present':
            continue
        else:
            failed_services.append(service_name)

    if failed_services:
        message = f"Services failed to restart: {', '.join(sorted(failed_services))}"
    elif restarted_services:
        message = f"Services restarted successfully: {', '.join(sorted(restarted_services))}"
    else:
        message = 'No restartable services were active in the current Dawn runtime.'

    return {
        'ok': not failed_services,
        'message': message,
        'services': normalized_results,
        'warnings': [str(item) for item in warnings],
    }


def cmd_im_status(args: argparse.Namespace) -> dict[str, Any]:
    statuses: dict[str, Any] = {}
    im_types = [args.im_type] if args.im_type else _detected_im_types()
    if not im_types:
        raise DawnImError(
            'No active Dawn IM connectors detected. Run scripts/validate.py or pass --im-type explicitly.'
        )
    for im in im_types:
        result = _exec_im_command(im, 'get_status', timeout_ms=args.timeout_ms)
        statuses[im] = result
    return {'ok': True, 'status': statuses}


def cmd_im_list_channels(args: argparse.Namespace) -> dict[str, Any]:
    if args.im_type:
        channels = _list_channels_once(args.im_type, timeout_ms=args.timeout_ms)
        return {'ok': True, 'channels': {args.im_type: channels}}

    all_channels: dict[str, Any] = {}
    im_types = _detected_im_types()
    if not im_types:
        raise DawnImError(
            'No active Dawn IM connectors detected. Run scripts/validate.py or pass --im-type explicitly.'
        )
    for im in im_types:
        all_channels[im] = _list_channels_once(im, timeout_ms=args.timeout_ms)
    return {'ok': True, 'channels': all_channels}


def cmd_im_send(args: argparse.Namespace) -> dict[str, Any]:
    result = _exec_im_command(
        args.im_type,
        'send_message',
        channel=args.channel,
        timeout_ms=args.timeout_ms,
        legacy_fields={
            'text': args.text,
            'replyToMessageId': args.reply_to,
            'avatarId': getattr(args, 'avatar_id', '') or None,
        },
    )
    if not result.get('success'):
        raise DawnImError(result.get('error', 'send_message failed'))
    return {'ok': True, 'result': result}


def cmd_im_react(args: argparse.Namespace) -> dict[str, Any]:
    result = _exec_im_command(
        args.im_type,
        'react',
        channel=args.channel,
        timeout_ms=args.timeout_ms,
        legacy_fields={'messageId': args.message_id, 'emoji': args.emoji},
    )
    if not result.get('success'):
        raise DawnImError(result.get('error', 'react failed'))
    return {'ok': True, 'result': result}


def cmd_im_register_channel(args: argparse.Namespace) -> dict[str, Any]:
    result = _exec_im_command(
        args.im_type,
        'register_channel',
        channel=args.channel_id,
        timeout_ms=args.timeout_ms,
        legacy_fields={
            'channelId': args.channel_id,
            'avatarId': args.avatar_id,
            'name': args.display_name,
            'text': args.display_name,
        },
    )
    if not result.get('success'):
        raise DawnImError(result.get('error', 'register_channel failed'))

    data = result.get('data') or {}
    folder = str(data.get('folder') or '')
    if not folder:
        base = re.sub(r'[^a-z0-9]+', '-', (args.display_name or args.channel_id).lower()).strip('-')
        folder = f"{args.im_type}-{base or 'channel'}"

    _add_group_binding(args.avatar_id, folder, args.im_type, args.display_name or folder)

    cfg = _read_json(_avatar_config_path(args.avatar_id), {})
    groups = cfg.get('groups') if isinstance(cfg.get('groups'), list) else []
    if folder not in groups:
        raise DawnImError('verification_failed: folder missing from avatar config groups')

    channels = _list_channels_once(args.im_type, timeout_ms=args.timeout_ms)
    if folder not in channels:
        raise DawnImError('verification_failed: folder missing from list_channels after register')

    return {'ok': True, 'folder': folder, 'result': result}


def cmd_im_create_channel(args: argparse.Namespace) -> dict[str, Any]:
    force_current = bool(getattr(args, 'current', False))
    prefer_current_avatar = bool(getattr(args, 'prefer_current_avatar', False))
    current_ctx = _resolve_current_context(getattr(args, 'thread_id', '')) if force_current else _try_resolve_current_context(getattr(args, 'thread_id', ''))

    im_type = _resolve_with_current(
        'im-type',
        str(args.im_type or '').strip(),
        (current_ctx or {}).get('imType', ''),
        force_current,
    )
    if not im_type:
        raise DawnImError('im_type is required, or use --current inside a Dawn IM conversation')
    _require_im_type(im_type)

    current_avatar_id = str((current_ctx or {}).get('avatarId', '') or '').strip()
    explicit_avatar_id = str(getattr(args, 'avatar_id', '') or '').strip()
    avatar_id = _resolve_with_current(
        'avatar-id',
        '',
        current_avatar_id if (force_current or prefer_current_avatar) else '',
        force_current,
    )
    if not avatar_id:
        avatar_id = explicit_avatar_id
    if not avatar_id and im_type == 'feishu':
        current_group = str((current_ctx or {}).get('groupFolder', '') or '').strip()
        if current_group:
            avatar_id = _find_avatar_for_group(current_group)
    if not avatar_id and str(args.source_group or '').strip():
        avatar_id = _find_avatar_for_group(str(args.source_group or '').strip())
    if not avatar_id:
        avatar_id = _resolve_avatar_id(args)

    source_group = str(args.source_group or '').strip()
    if im_type == 'feishu' and not source_group and not args.members:
        source_group = str((current_ctx or {}).get('groupFolder', '') or '').strip()

    allow_private_chat = _coerce_bool(
        getattr(args, 'allow_private_chat', None),
        default=(im_type == 'feishu'),
    )
    folder = _generate_unique_folder(im_type, args.name)
    params: dict[str, Any] = {}
    source_group_used = ''
    auto_selected_source_group = ''
    if im_type == 'feishu':
        params, source_group_used, auto_selected = _prepare_feishu_create_params(
            avatar_id,
            args.members,
            source_group,
            args.timeout_ms,
        )
        if auto_selected:
            auto_selected_source_group = source_group_used
        if allow_private_chat:
            params['allowPrivateChat'] = True
    else:
        if args.members:
            params['members'] = args.members
        if source_group:
            params['sourceGroup'] = source_group

    create_result = _exec_im_command(
        im_type,
        'create_channel',
        timeout_ms=args.timeout_ms,
        params=params or None,
        legacy_fields={
            'name': args.name,
            'text': args.name,
            'folder': folder,
            'channel': '',
            'avatarId': avatar_id,
        },
    )
    if not create_result.get('success'):
        raise DawnImError(create_result.get('error', 'create_channel failed'))

    bind_result = _add_group_binding(avatar_id, folder, im_type, args.name)

    channels = _list_channels_once(im_type, timeout_ms=args.timeout_ms)
    if folder not in channels:
        raise DawnImError('verification_failed: created folder missing from list_channels')

    cfg = _read_json(_avatar_config_path(avatar_id), {})
    gc = ((cfg.get('groupConfigs') or {}).get(folder) or {})
    groups = cfg.get('groups') if isinstance(cfg.get('groups'), list) else []
    if folder not in groups or gc.get('imType') != im_type:
        raise DawnImError('verification_failed: config binding mismatch after create_channel')

    result = {
        'ok': True,
        'message': f"Channel '{args.name}' created and bound to avatar '{avatar_id}'.",
        'channel': {'name': args.name, 'folder': folder, 'imType': im_type},
        'avatarId': avatar_id,
        'bindResult': bind_result,
        'createResult': create_result,
    }
    if not source_group_used:
        source_group_used = str(params.get('sourceGroup') or '').strip()
    if source_group_used:
        result['sourceGroupUsed'] = source_group_used
        if auto_selected_source_group:
            result['sourceGroupAutoSelected'] = True
    return result


def cmd_im_delete_channel(args: argparse.Namespace) -> dict[str, Any]:
    folder = args.folder
    if folder.lower() == 'main':
        raise DawnImError("Refusing to delete reserved folder 'main'")

    if not re.match(r'^(feishu|discord|whatsapp)-', folder):
        resolved = _find_folder_by_display_name(folder, args.im_type)
        if resolved:
            folder = resolved

    delete_result = _exec_im_command(
        args.im_type,
        'delete_channel',
        channel=folder,
        timeout_ms=args.timeout_ms,
        legacy_fields={'leaveService': bool(args.leave_service)},
    )

    if args.leave_service and delete_result.get('success') is False:
        raise DawnImError(f"delete_channel failed on service side: {delete_result.get('error', 'unknown error')}")

    if delete_result.get('success') is False and delete_result.get('error') == 'Timed out waiting for IM service response':
        raise DawnImError('IM service unreachable. Channel may still be active. Retry when service is online.')

    unbind_result = _remove_group_bindings(folder, args.avatar_id)
    _remove_group_thread_state(folder)

    cfg_check_avatar = args.avatar_id or (_find_avatar_for_group(folder) or '')
    if cfg_check_avatar:
        cfg = _read_json(_avatar_config_path(cfg_check_avatar), {})
        groups = cfg.get('groups') if isinstance(cfg.get('groups'), list) else []
        if folder in groups:
            raise DawnImError('verification_failed: folder still present in avatar groups after delete')

    channels = _list_channels_once(args.im_type, timeout_ms=args.timeout_ms)
    if folder in channels:
        raise DawnImError('verification_failed: folder still present in list_channels after delete')

    return {
        'ok': True,
        'message': (
            f"Channel '{folder}' deleted and bindings removed."
            if delete_result.get('success')
            else f"Channel '{folder}' bindings removed (bridge reported: {delete_result.get('error', 'error')})."
        ),
        'imType': args.im_type,
        'folder': folder,
        'leaveService': bool(args.leave_service),
        'deleteResult': delete_result,
        'unbindResult': unbind_result,
    }


def cmd_im_action(args: argparse.Namespace) -> dict[str, Any]:
    raw_params = json.loads(args.params_json) if args.params_json else {}
    if not isinstance(raw_params, dict):
        raise DawnImError('params-json must decode to a JSON object')

    # Handle feishu app config actions locally (not via IPC)
    if args.im_type == 'feishu':
        action = _normalize_action(args.action)
        if action == 'app-add':
            return _feishu_app_add(raw_params)
        if action == 'app-remove':
            return _feishu_app_remove(raw_params)
        if action == 'app-list':
            return _feishu_app_list()

    action = _normalize_action(args.action)
    legacy_fields = dict(raw_params)
    result = _exec_im_command(
        args.im_type,
        action,
        channel=args.channel or '',
        timeout_ms=args.timeout_ms,
        params=raw_params,
        legacy_fields=legacy_fields,
    )
    if not isinstance(result, dict) or 'success' not in result:
        raise DawnImError('Missing command result')
    return {'ok': bool(result.get('success')), 'result': result}


def _load_feishu_connector_config() -> dict[str, Any]:
    config = _read_json(FEISHU_CONNECTOR_CONFIG_PATH, {})
    if isinstance(config, dict) and isinstance(config.get('apps'), list):
        return config
    if isinstance(config, dict) and (
        str(config.get('appId') or '').strip()
        or str(config.get('appSecret') or '').strip()
        or str(config.get('secretRef') or '').strip()
    ):
        return {
            'apps': [{
                'name': str(config.get('name') or 'default').strip() or 'default',
                'appId': str(config.get('appId') or '').strip(),
                'appSecret': str(config.get('appSecret') or '').strip(),
                'secretRef': str(config.get('secretRef') or '').strip(),
                'defaultAvatarId': str(config.get('defaultAvatarId') or config.get('avatarId') or '').strip(),
                'domain': str(config.get('domain') or '').strip(),
            }],
        }
    return {'apps': []}


def _feishu_app_add(params: dict[str, Any]) -> dict[str, Any]:
    """Add a new Feishu app to ~/.dawn/connectors/feishu/config.json."""
    name = params.get('name')
    app_id = params.get('appId')
    app_secret = str(params.get('appSecret') or '').strip()
    secret_ref = str(params.get('secretRef') or '').strip()
    default_avatar_id = params.get('defaultAvatarId', '')

    if not name or not app_id or (not app_secret and not secret_ref):
        raise DawnImError('app_add requires name, appId, and either appSecret or secretRef')

    config = _load_feishu_connector_config()

    # Check if app already exists
    for app in config['apps']:
        if app.get('name') == name:
            raise DawnImError(f'App with name "{name}" already exists')
        if app.get('appId') == app_id:
            raise DawnImError(f'App with appId "{app_id}" already exists')

    new_app = {
        'name': name,
        'appId': app_id,
        'defaultAvatarId': default_avatar_id,
    }
    if app_secret:
        new_app['appSecret'] = app_secret
    if secret_ref:
        new_app['secretRef'] = secret_ref
    config['apps'].append(new_app)
    _write_json_atomic(FEISHU_CONNECTOR_CONFIG_PATH, config)

    return {
        'ok': True,
        'message': f'App "{name}" added. Run service-restart --service dawn-feishu to apply.',
        'app': new_app,
        'configPath': str(FEISHU_CONNECTOR_CONFIG_PATH),
    }


def _feishu_app_remove(params: dict[str, Any]) -> dict[str, Any]:
    """Remove a Feishu app from ~/.dawn/connectors/feishu/config.json."""
    name = params.get('name')
    if not name:
        raise DawnImError('app_remove requires name')

    config = _load_feishu_connector_config()

    original_count = len(config['apps'])
    config['apps'] = [app for app in config['apps'] if app.get('name') != name]

    if len(config['apps']) == original_count:
        raise DawnImError(f'App "{name}" not found')

    _write_json_atomic(FEISHU_CONNECTOR_CONFIG_PATH, config)

    return {
        'ok': True,
        'message': f'App "{name}" removed. Run service-restart --service dawn-feishu to apply.',
        'configPath': str(FEISHU_CONNECTOR_CONFIG_PATH),
    }


def _feishu_app_list() -> dict[str, Any]:
    """List all configured Feishu apps."""
    config = _load_feishu_connector_config()
    apps = config.get('apps', [])
    # Hide secrets in output
    safe_apps = [
        {
            'name': app.get('name'),
            'appId': app.get('appId'),
            'defaultAvatarId': app.get('defaultAvatarId', ''),
            'usesSecretRef': bool(str(app.get('secretRef') or '').strip()),
        }
        for app in apps
    ]
    return {
        'ok': True,
        'count': len(safe_apps),
        'apps': safe_apps,
        'configPath': str(FEISHU_CONNECTOR_CONFIG_PATH),
    }


def cmd_background_dispatch(args: argparse.Namespace) -> dict[str, Any]:
    try:
        raw = json.loads(args.items_json or '[]')
    except Exception as err:
        raise DawnImError(f'items-json parse failed: {err}')

    request_job_id = str(getattr(args, 'job_id', '') or '').strip()
    created_at = _now_iso()
    items_raw: Any

    if isinstance(raw, list):
        items_raw = raw
    elif isinstance(raw, dict):
        items_raw = raw.get('items')
        if not isinstance(items_raw, list):
            raise DawnImError('items-json object must include an items array')
        if not request_job_id:
            request_job_id = str(raw.get('jobId') or raw.get('job_id') or raw.get('id') or '').strip()
    else:
        raise DawnImError('items-json must decode to a JSON array or an object with items')

    if not items_raw:
        raise DawnImError('background-dispatch requires at least one item')

    job_id = request_job_id or f'bg_job_{int(time.time() * 1000)}_{uuid.uuid4().hex[:6]}'
    items = [_normalize_background_item(item, index) for index, item in enumerate(items_raw)]

    payload = {
        'jobId': job_id,
        'createdAt': created_at,
        'items': items,
    }

    BACKGROUND_REQUESTS_DIR.mkdir(parents=True, exist_ok=True)
    request_path = BACKGROUND_REQUESTS_DIR / f"{int(time.time() * 1000)}-{uuid.uuid4().hex[:8]}.json"
    _write_json_atomic(request_path, payload)

    job = _poll_background_job(job_id, timeout_ms=args.timeout_ms)
    response = {
        'ok': True,
        'jobId': job_id,
        'itemCount': len(items),
        'requestPath': str(request_path),
    }
    if job:
        response['job'] = job
        response['summary'] = _summarize_background_job(job)
    else:
        response['message'] = 'Background job submitted; waiting for Dawn Settings to accept it.'
    return response


def cmd_background_status(args: argparse.Namespace) -> dict[str, Any]:
    job_id = str(args.job_id or '').strip()
    if not job_id:
        raise DawnImError('job-id is required')

    job = _poll_background_job(job_id, timeout_ms=args.timeout_ms)
    if not job:
        raise DawnImError(f'Background job not found: {job_id}')

    return {
        'ok': True,
        'jobId': job_id,
        'summary': _summarize_background_job(job),
        'job': job,
    }


def cmd_context_current(args: argparse.Namespace) -> dict[str, Any]:
    context = _resolve_current_context(getattr(args, 'thread_id', ''))
    return {
        'ok': True,
        'context': {
            'threadId': context['threadId'],
            'groupFolder': context['groupFolder'],
            'avatarId': context['avatarId'],
            'chatJid': context['chatJid'],
            'imType': context['imType'],
            'expiresAt': context['expiresAt'],
        },
    }


def cmd_codex_new_chat(args: argparse.Namespace) -> dict[str, Any]:
    group = str(args.group or '').strip()
    force_current = bool(getattr(args, 'current', False))
    current_ctx = _resolve_current_context(getattr(args, 'thread_id', '')) if (force_current or not group) else None

    group = _resolve_with_current('group', group, (current_ctx or {}).get('groupFolder', ''), force_current)
    if not group:
        raise DawnImError('group is required, or use --current inside a Dawn IM conversation')

    same_group_as_current = bool(current_ctx and group == str(current_ctx.get('groupFolder') or '').strip())
    im_type = _resolve_with_current(
        'im-type',
        str(args.im_type or '').strip(),
        (current_ctx or {}).get('imType', '') if same_group_as_current else '',
        force_current,
    )
    if not im_type:
        im_type = _resolve_im_type_for_group(group)
    if not im_type:
        raise DawnImError(
            'Could not infer im_type for the target group. Pass --im-type or ensure the group binding exists under ~/.dawn/avatars.'
        )
    _require_im_type(im_type)

    avatar_id = _resolve_with_current(
        'avatar-id',
        str(args.avatar_id or '').strip(),
        (current_ctx or {}).get('avatarId', '') if same_group_as_current else '',
        force_current,
    )
    if not avatar_id:
        avatar_id = _find_avatar_for_group(group)
    if not avatar_id:
        raise DawnImError('avatar_id is required when group is not bound to any avatar')

    request_id = str(uuid.uuid4())
    _write_ipc_request(im_type, {
        'id': request_id,
        'type': 'new_chat',
        'avatarId': avatar_id,
        'groupFolder': group,
        'timestamp': _now_iso(),
    })
    result = _poll_command_response(im_type, request_id, timeout_ms=args.timeout_ms)
    if not result.get('ok'):
        raise DawnImError(result.get('message', 'new_chat failed'))
    if not result.get('threadId'):
        raise DawnImError('verification_failed: new_chat response missing threadId')
    return {'ok': True, 'threadId': result.get('threadId'), 'result': result}


def cmd_codex_fork_chat(args: argparse.Namespace) -> dict[str, Any]:
    source_group = str(args.source_group or '').strip()
    source_thread_id = str(args.source_thread_id or '').strip()
    force_current = bool(getattr(args, 'current', False))
    needs_current = force_current or not source_group or not source_thread_id
    current_ctx = _resolve_current_context(getattr(args, 'thread_id', '')) if needs_current else None

    source_group = _resolve_with_current('source-group', source_group, (current_ctx or {}).get('groupFolder', ''), force_current)
    if not source_group:
        raise DawnImError('source_group is required, or use --current inside a Dawn IM conversation')

    same_group_as_current = bool(current_ctx and source_group == str(current_ctx.get('groupFolder') or '').strip())

    im_type = _resolve_with_current(
        'im-type',
        str(args.im_type or '').strip(),
        (current_ctx or {}).get('imType', '') if same_group_as_current else '',
        force_current,
    )
    if not im_type:
        im_type = _resolve_im_type_for_group(source_group)
    if not im_type:
        raise DawnImError(
            'Could not infer im_type for the source group. Pass --im-type or ensure the group binding exists under ~/.dawn/avatars.'
        )
    _require_im_type(im_type)

    avatar_id = _resolve_with_current(
        'avatar-id',
        str(args.avatar_id or '').strip(),
        (current_ctx or {}).get('avatarId', '') if same_group_as_current else '',
        force_current,
    )
    if not avatar_id:
        avatar_id = _find_avatar_for_group(source_group)
    if not avatar_id:
        raise DawnImError('avatar_id is required when source_group is not bound to any avatar')

    source_thread_id = _resolve_with_current(
        'source-thread-id',
        source_thread_id,
        (current_ctx or {}).get('threadId', '') if same_group_as_current else '',
        force_current,
    )

    folder = _generate_unique_folder(im_type, args.name)
    create_params: Optional[dict[str, Any]] = None
    source_group_used = ''
    source_group_auto_selected = False
    if im_type == 'feishu':
        create_params, source_group_used, source_group_auto_selected = _prepare_feishu_create_params(
            avatar_id,
            [],
            source_group,
            args.timeout_ms,
        )

    create_result = _exec_im_command(
        im_type,
        'create_channel',
        timeout_ms=args.timeout_ms,
        params=create_params,
        legacy_fields={
            'name': args.name,
            'text': args.name,
            'folder': folder,
            'avatarId': avatar_id,
        },
    )
    if not create_result.get('success'):
        raise DawnImError(create_result.get('error', 'fork create_channel failed'))

    bind_result = _add_group_binding(avatar_id, folder, im_type, args.name, inherit_group=source_group)

    request_id = str(uuid.uuid4())
    _write_ipc_request(im_type, {
        'id': request_id,
        'type': 'fork_chat',
        'avatarId': avatar_id,
        'groupFolder': folder,
        'sourceThreadId': source_thread_id,
        'timestamp': _now_iso(),
    })

    result = _poll_command_response(im_type, request_id, timeout_ms=args.timeout_ms)
    if not result.get('ok'):
        raise DawnImError(result.get('message', 'fork_chat failed'))
    if not result.get('threadId'):
        raise DawnImError('verification_failed: fork_chat response missing threadId')

    channels = _list_channels_once(im_type, timeout_ms=args.timeout_ms)
    if folder not in channels:
        raise DawnImError('verification_failed: forked channel missing from list_channels')

    response = {
        'ok': True,
        'message': f"Forked: channel '{args.name}' created with conversation history. Folder: {folder}",
        'channel': {'name': args.name, 'folder': folder, 'imType': im_type},
        'avatarId': avatar_id,
        'threadId': result.get('threadId'),
        'channelCreated': True,
        'bindResult': bind_result,
    }
    if source_group_used:
        response['sourceGroupUsed'] = source_group_used
        if source_group_auto_selected:
            response['sourceGroupAutoSelected'] = True
    return response


def _resolve_avatar_id(args: argparse.Namespace) -> str:
    avatar_id = getattr(args, 'avatar_id', None) or ''
    if avatar_id:
        return avatar_id
    global_path = DAWN_HOME / 'global.json'
    g = _read_json(global_path, {})
    avatar_id = g.get('activeAvatar', '')
    if not avatar_id:
        raise DawnImError('No --avatar-id provided and no active avatar set in global.json')
    return avatar_id


APPROVAL_POLICIES = ('never', 'on-request', 'on-failure', 'untrusted')
SANDBOX_POLICIES = ('workspace-write', 'danger-full-access', 'read-only', 'external-sandbox')
CODEX_READ_SCOPES = ('default', 'group', 'effective')
CODEX_SET_SCOPES = ('default', 'group')
CODEX_CONFIG_FIELD_TO_ARG = {
    'model': 'model',
    'modelProvider': 'model_provider',
    'approvalPolicy': 'approval_policy',
    'sandboxPolicy': 'sandbox_policy',
    'workingDirectory': 'working_directory',
    'baseInstructions': 'base_instructions',
    'developerInstructions': 'developer_instructions',
}
GROUP_CODEX_FIELDS = ('model', 'modelProvider', 'approvalPolicy', 'sandboxPolicy', 'workingDirectory')


def _normalize_codex_dict(raw: Any, group_override: bool = False) -> dict[str, Any]:
    if not isinstance(raw, dict):
        return {}
    allowed_fields = GROUP_CODEX_FIELDS if group_override else tuple(CODEX_CONFIG_FIELD_TO_ARG.keys())
    normalized: dict[str, Any] = {}
    for field in allowed_fields:
        value = raw.get(field)
        if value is None:
            continue
        if isinstance(value, str) and group_override and not value.strip():
            continue
        normalized[field] = value
    return normalized


def _merge_codex_effective(default_codex: dict[str, Any], group_codex: dict[str, Any]) -> dict[str, Any]:
    merged = dict(_normalize_codex_dict(default_codex))
    for field, value in _normalize_codex_dict(group_codex, group_override=True).items():
        merged[field] = value
    return merged


def _load_avatar_default_codex(avatar_id: str) -> dict[str, Any]:
    return _normalize_codex_dict(_read_json(_avatar_config_path(avatar_id), {}).get('codex', {}))



def _provider_from_model_id(model_id: str) -> str:
    return model_id.split('/', 1)[0].strip() if '/' in model_id else ''


def _model_tail(model_id: str) -> str:
    return model_id.split('/', 1)[1].strip() if '/' in model_id else model_id.strip()


def _load_codex_capabilities_cache() -> dict[str, Any]:
    payload = _read_json(CAPABILITIES_CACHE_PATH, {})
    if not isinstance(payload, dict):
        return {}
    models = payload.get('models') if isinstance(payload.get('models'), list) else []
    providers = payload.get('providers') if isinstance(payload.get('providers'), list) else []
    return {'models': models, 'providers': providers}


def _codex_home() -> Path:
    return DAWN_HOME / '.codex'


def _read_json_string_map(path: Path) -> dict[str, str]:
    payload = _read_json(path, {})
    if not isinstance(payload, dict):
        return {}
    result: dict[str, str] = {}
    for key, value in payload.items():
        normalized_key = str(key or '').strip()
        if not normalized_key:
            continue
        result[normalized_key] = str(value or '').strip()
    return result


def _load_provider_secrets() -> dict[str, str]:
    return _read_json_string_map(_codex_home() / 'provider-secrets.json')


def _load_provider_env_map() -> dict[str, str]:
    return _read_json_string_map(_codex_home() / 'provider-env-map.json')


def _read_proxy_url_from_config() -> str:
    path = DAWN_HOME / 'proxy.conf'
    if not path.exists():
        return ''
    try:
        explicit_url = ''
        proxy_port = ''
        for raw_line in path.read_text(encoding='utf-8').splitlines():
            line = raw_line.strip()
            if not line or line.startswith('#') or '=' not in line:
                continue
            key, raw_value = line.split('=', 1)
            key = key.strip()
            value = raw_value.strip().strip('"').strip("'")
            if not value:
                continue
            if key in {'PROXY_URL', 'HTTP_PROXY', 'HTTPS_PROXY'}:
                explicit_url = value
            elif key == 'PROXY_PORT':
                proxy_port = value
        if explicit_url:
            return explicit_url
        if proxy_port.isdigit() and int(proxy_port) > 0:
            return f'http://127.0.0.1:{proxy_port}'
    except Exception:
        return ''
    return ''


def _resolve_codex_app_server_bin() -> str:
    override = str(os.environ.get('CODEX_APP_SERVER_BIN', '') or '').strip()
    if override and Path(override).exists():
        return override
    binary_name = 'codex-app-server.exe' if os.name == 'nt' else 'codex-app-server'
    runtime_slot = _active_runtime_slot_dir()
    runtime_candidates = [
        runtime_slot / binary_name if runtime_slot else None,
        RUNTIME_DIR / binary_name,
    ]
    for candidate in runtime_candidates:
        if candidate and candidate.exists():
            return str(candidate)
    return binary_name


def _rpc_error_message(error_value: Any) -> str:
    if isinstance(error_value, dict):
        message = str(error_value.get('message', '') or '').strip()
        if message:
            return message
    return str(error_value)


def _request_codex_capabilities_live() -> dict[str, Any]:
    env = os.environ.copy()
    env['CODEX_HOME'] = str(_codex_home())
    env['CODEX_EMBEDDED_MODE'] = '1'

    for key in ('HTTP_PROXY', 'HTTPS_PROXY', 'http_proxy', 'https_proxy', 'ALL_PROXY', 'all_proxy'):
        env.pop(key, None)
    proxy_url = _read_proxy_url_from_config()
    if proxy_url:
        for key in ('HTTP_PROXY', 'HTTPS_PROXY', 'http_proxy', 'https_proxy', 'ALL_PROXY', 'all_proxy'):
            env[key] = proxy_url

    provider_env_map = _load_provider_env_map()
    for provider_id, api_key in _load_provider_secrets().items():
        env_key = str(provider_env_map.get(provider_id, '') or '').strip()
        if env_key and api_key.strip():
            env[env_key] = api_key.strip()

    proc = subprocess.Popen(
        [_resolve_codex_app_server_bin()],
        stdin=subprocess.PIPE,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True,
        encoding='utf-8',
        env=env,
        bufsize=1,
    )
    stderr_lines: list[str] = []

    def send(payload: dict[str, Any]) -> None:
        assert proc.stdin is not None
        proc.stdin.write(json.dumps(payload, ensure_ascii=False) + '\n')
        proc.stdin.flush()

    def read_response(request_id: str, method: str, timeout_sec: float) -> Any:
        assert proc.stdout is not None
        assert proc.stderr is not None
        deadline = time.time() + timeout_sec
        while time.time() < deadline:
            if proc.poll() is not None:
                break
            ready, _, _ = select.select([proc.stdout, proc.stderr], [], [], 0.2)
            for stream in ready:
                line = stream.readline()
                if not line:
                    continue
                if stream is proc.stderr:
                    stripped = line.strip()
                    if stripped:
                        stderr_lines.append(stripped)
                    continue
                try:
                    message = json.loads(line)
                except Exception:
                    continue
                if str(message.get('id', '')) != request_id:
                    continue
                if 'error' in message:
                    raise DawnImError(_rpc_error_message(message['error']))
                return message.get('result')
        detail = '; '.join(stderr_lines[-5:])
        if detail:
            raise DawnImError(f'Timed out waiting for codex response: {method}. stderr: {detail}')
        raise DawnImError(f'Timed out waiting for codex response: {method}')

    try:
        send({
            'id': '1',
            'method': 'initialize',
            'params': {
                'clientInfo': {
                    'name': 'dawn-im-management',
                    'title': 'Dawn IM Management',
                    'version': '0.1.0',
                },
                'capabilities': {
                    'experimentalApi': True,
                },
            },
        })
        read_response('1', 'initialize', 25.0)
        send({'method': 'initialized', 'params': None})

        send({'id': '2', 'method': 'provider/list', 'params': {}})
        providers_result = read_response('2', 'provider/list', 25.0)
        send({'id': '3', 'method': 'model/list', 'params': {}})
        models_result = read_response('3', 'model/list', 25.0)
    finally:
        if proc.stdin is not None:
            try:
                proc.stdin.close()
            except Exception:
                pass
        try:
            proc.terminate()
            proc.wait(timeout=5)
        except Exception:
            try:
                proc.kill()
            except Exception:
                pass

    raw_providers = providers_result.get('data') if isinstance(providers_result, dict) else []
    raw_models = models_result.get('data') if isinstance(models_result, dict) else []
    providers: list[dict[str, Any]] = []
    for item in raw_providers if isinstance(raw_providers, list) else []:
        if not isinstance(item, dict):
            continue
        provider_id = str(item.get('id', '') or '').strip()
        if not provider_id:
            continue
        providers.append(dict(item))
    models: list[dict[str, Any]] = []
    for item in raw_models if isinstance(raw_models, list) else []:
        if not isinstance(item, dict):
            continue
        model_id = str(item.get('id', '') or '').strip()
        if not model_id:
            continue
        normalized = dict(item)
        normalized['model'] = str(item.get('model', model_id) or model_id).strip()
        models.append(normalized)

    if not providers or not models:
        raise DawnImError('Codex model/provider registry is unavailable. Open Dawn Settings or reconnect Codex before changing model settings.')

    env_map = {
        str(item.get('id', '') or '').strip(): str(item.get('envKey', '') or '').strip()
        for item in providers
        if str(item.get('id', '') or '').strip() and str(item.get('envKey', '') or '').strip()
    }
    if env_map:
        _write_json_atomic(_codex_home() / 'provider-env-map.json', env_map)

    payload = {
        'updatedAt': _now_iso(),
        'providers': providers,
        'models': models,
    }
    _write_json_atomic(CAPABILITIES_CACHE_PATH, payload)
    return {'providers': providers, 'models': models}


def _load_codex_capabilities() -> dict[str, Any]:
    cached = _load_codex_capabilities_cache()
    cached_models = cached.get('models') if isinstance(cached.get('models'), list) else []
    cached_providers = cached.get('providers') if isinstance(cached.get('providers'), list) else []
    if cached_models and cached_providers:
        return {'models': cached_models, 'providers': cached_providers}
    return _request_codex_capabilities_live()


_KNOWN_MODEL_PROVIDER_MAP: Optional[dict[str, list[str]]] = None


def _collect_model_provider_pairs(payload: Any, pairs: dict[str, set[str]]) -> None:
    if isinstance(payload, dict):
        model = str(payload.get('model', '') or '').strip()
        provider = str(payload.get('modelProvider', '') or '').strip()
        if model and provider:
            pairs.setdefault(model, set()).add(provider)
        for value in payload.values():
            _collect_model_provider_pairs(value, pairs)
        return
    if isinstance(payload, list):
        for item in payload:
            _collect_model_provider_pairs(item, pairs)


def _known_model_provider_map() -> dict[str, list[str]]:
    global _KNOWN_MODEL_PROVIDER_MAP
    if _KNOWN_MODEL_PROVIDER_MAP is not None:
        return _KNOWN_MODEL_PROVIDER_MAP
    pairs: dict[str, set[str]] = {}
    if AVATARS_DIR.exists():
        for avatar_dir in AVATARS_DIR.iterdir():
            if not avatar_dir.is_dir():
                continue
            _collect_model_provider_pairs(_read_json(avatar_dir / 'config.json', {}), pairs)
            _collect_model_provider_pairs(_read_json(avatar_dir / 'state.json', {}), pairs)
    _KNOWN_MODEL_PROVIDER_MAP = {
        model: sorted(providers)
        for model, providers in pairs.items()
        if model and providers
    }
    return _KNOWN_MODEL_PROVIDER_MAP


def _candidate_providers_for_model(model_entry: dict[str, Any]) -> list[str]:
    providers: set[str] = set()
    model_id = str(model_entry.get('id', '') or '').strip()
    model_name = str(model_entry.get('model', model_id) or model_id).strip()
    for key in ('provider', 'providerId', 'modelProvider'):
        value = str(model_entry.get(key, '') or '').strip()
        if value:
            providers.add(value)
    id_provider = _provider_from_model_id(model_id)
    if id_provider:
        providers.add(id_provider)
    for candidate in {model_name, model_id, _model_tail(model_id)}:
        if not candidate:
            continue
        for provider in _known_model_provider_map().get(candidate, []):
            providers.add(provider)
    return sorted(providers)


def _all_known_model_labels(models: list[dict[str, Any]]) -> list[str]:
    labels: set[str] = set()
    for model in models:
        if not isinstance(model, dict):
            continue
        model_id = str(model.get('id', '') or '').strip()
        model_name = str(model.get('model', model_id) or model_id).strip()
        if not model_name and not model_id:
            continue
        providers = _candidate_providers_for_model(model)
        label = model_name or model_id
        if len(providers) > 1:
            for provider in providers:
                labels.add(f'{provider}/{label}')
        else:
            labels.add(label)
    return sorted(labels)


def _normalize_approval_policy_value(value: str, *, allow_empty: bool) -> Optional[str]:
    trimmed = str(value or '').strip()
    if not trimmed:
        return None if allow_empty else 'never'
    mapping = {
        'never': 'never',
        'onrequest': 'on-request',
        'on-request': 'on-request',
        'onfailure': 'on-failure',
        'on-failure': 'on-failure',
        'unlesstrusted': 'untrusted',
        'unless_trusted': 'untrusted',
        'unless-trusted': 'untrusted',
        'untrusted': 'untrusted',
    }
    normalized = mapping.get(trimmed.replace('_', '-').lower())
    if not normalized:
        raise DawnImError(
            f"Invalid approvalPolicy '{value}'. Must be one of: {', '.join(APPROVAL_POLICIES)}"
        )
    return normalized


def _normalize_sandbox_policy_value(value: str, *, allow_empty: bool) -> Optional[str]:
    trimmed = str(value or '').strip()
    if not trimmed:
        return None if allow_empty else 'workspace-write'
    mapping = {
        'workspacewrite': 'workspace-write',
        'workspace-write': 'workspace-write',
        'dangerfullaccess': 'danger-full-access',
        'danger-full-access': 'danger-full-access',
        'readonly': 'read-only',
        'read-only': 'read-only',
        'externalsandbox': 'external-sandbox',
        'external-sandbox': 'external-sandbox',
    }
    normalized = mapping.get(trimmed.replace('_', '-').lower())
    if not normalized:
        raise DawnImError(
            f"Invalid sandboxPolicy '{value}'. Must be one of: {', '.join(SANDBOX_POLICIES)}"
        )
    return normalized


def _normalize_working_directory_value(value: str, *, allow_empty: bool) -> Optional[str]:
    trimmed = str(value or '').strip()
    if not trimmed:
        return None if allow_empty else ''
    expanded = Path(os.path.expanduser(trimmed))
    if not expanded.is_absolute():
        raise DawnImError(f"workingDirectory must be an absolute path after expansion: {trimmed}")
    if not expanded.exists():
        raise DawnImError(f"workingDirectory does not exist: {expanded}")
    if not expanded.is_dir():
        raise DawnImError(f"workingDirectory must point to a directory: {expanded}")
    return str(expanded.resolve())


def _normalize_prompt_path_value(avatar_id: str, field_name: str, value: str) -> str:
    trimmed = str(value or '').strip()
    if not trimmed:
        return ''
    raw_path = Path(trimmed)
    if not raw_path.is_absolute() and '..' in raw_path.parts:
        raise DawnImError(f"{field_name} cannot escape the avatar directory: {trimmed}")
    resolved = raw_path if raw_path.is_absolute() else AVATARS_DIR / avatar_id / raw_path
    if not resolved.exists():
        raise DawnImError(f"{field_name} file does not exist: {resolved}")
    if resolved.is_dir():
        raise DawnImError(f"{field_name} must point to a file, not a directory: {resolved}")
    return str(resolved.resolve()) if raw_path.is_absolute() else trimmed


def _resolve_provider_id(providers: list[dict[str, Any]], submitted: str) -> str:
    trimmed = str(submitted or '').strip()
    if not trimmed:
        return ''
    for provider in providers:
        if str(provider.get('id', '')).strip() == trimmed:
            return trimmed
    suggestions = [
        str(provider.get('id', '')).strip()
        for provider in providers
        if trimmed and trimmed in str(provider.get('id', '')).strip()
    ][:5]
    if suggestions:
        raise DawnImError(
            f"Unknown modelProvider '{trimmed}'. Suggestions: {', '.join(suggestions)}"
        )
    raise DawnImError(f"Unknown modelProvider '{trimmed}'")


def _resolve_model_entry(models: list[dict[str, Any]], submitted: str, provider_hint: str = '') -> tuple[str, str]:
    trimmed = str(submitted or '').strip()
    matches: set[tuple[str, str]] = set()
    unresolved_match = False
    for model in models:
        model_id = str(model.get('id', '')).strip()
        model_name = str(model.get('model', model_id)).strip()
        if trimmed not in {model_name, model_id, _model_tail(model_id)}:
            continue
        candidate_providers = _candidate_providers_for_model(model)
        if provider_hint:
            if candidate_providers and provider_hint not in candidate_providers:
                continue
            matches.add((model_name, provider_hint))
            continue
        if len(candidate_providers) == 1:
            matches.add((model_name, candidate_providers[0]))
            continue
        if len(candidate_providers) > 1:
            for provider in candidate_providers:
                matches.add((model_name, provider))
            continue
        unresolved_match = True
    if len(matches) == 1:
        return next(iter(matches))
    if len(matches) > 1:
        candidates = sorted({f'{provider}/{model}' for model, provider in matches})
        raise DawnImError(
            f"Model '{trimmed}' is ambiguous. Specify modelProvider explicitly. Candidates: {', '.join(candidates)}"
        )
    if unresolved_match:
        raise DawnImError(
            f"Provider for model '{trimmed}' could not be inferred. Specify modelProvider explicitly."
        )
    known_models = _all_known_model_labels(models)
    if known_models:
        raise DawnImError(f"Unknown model '{trimmed}'. Known models: {', '.join(known_models)}")
    raise DawnImError(f"Unknown model '{trimmed}'")


def _validate_and_normalize_model_selection(model: str, model_provider: str) -> tuple[str, str]:
    trimmed_model = str(model or '').strip()
    trimmed_provider = str(model_provider or '').strip()
    if not trimmed_model and not trimmed_provider:
        return '', ''
    capabilities = _load_codex_capabilities()
    models = capabilities.get('models') if isinstance(capabilities.get('models'), list) else []
    providers = capabilities.get('providers') if isinstance(capabilities.get('providers'), list) else []
    if not models or not providers:
        raise DawnImError(
            'Codex model/provider registry is unavailable. Open Dawn Settings or reconnect Codex before changing model settings.'
        )
    canonical_provider = _resolve_provider_id(providers, trimmed_provider)
    if not trimmed_model:
        return '', canonical_provider
    canonical_model, inferred_provider = _resolve_model_entry(models, trimmed_model, canonical_provider)
    if canonical_provider and canonical_provider != inferred_provider:
        raise DawnImError(
            f"Model '{trimmed_model}' does not belong to provider '{canonical_provider}'"
        )
    return canonical_model, canonical_provider or inferred_provider


def _build_codex_views(avatar_id: str, group: str) -> dict[str, dict[str, Any]]:
    config = _read_json(_avatar_config_path(avatar_id), {})
    inherited_default_codex = _load_avatar_default_codex('_default') if avatar_id != '_default' else {}
    default_codex = _normalize_codex_dict(config.get('codex', {}))
    group_configs = config.get('groupConfigs') if isinstance(config.get('groupConfigs'), dict) else {}
    group_entry = group_configs.get(group) if group else {}
    group_codex = _normalize_codex_dict(
        group_entry.get('codex', {}) if isinstance(group_entry, dict) else {},
        group_override=True,
    )
    avatar_effective_codex = _merge_codex_effective(inherited_default_codex, default_codex)
    effective_codex = _merge_codex_effective(avatar_effective_codex, group_codex) if group else avatar_effective_codex
    return {
        'inheritedDefaultCodex': inherited_default_codex,
        'defaultCodex': default_codex,
        'groupCodex': group_codex,
        'avatarEffectiveCodex': avatar_effective_codex,
        'effectiveCodex': effective_codex,
    }


def _assess_effective_codex_validity(avatar_id: str, effective_codex: dict[str, Any]) -> tuple[str, list[str]]:
    warnings: list[str] = []
    unknown = False
    try:
        _normalize_approval_policy_value(str(effective_codex.get('approvalPolicy', 'never')), allow_empty=False)
        _normalize_sandbox_policy_value(str(effective_codex.get('sandboxPolicy', 'workspace-write')), allow_empty=False)
        _normalize_working_directory_value(str(effective_codex.get('workingDirectory', '')), allow_empty=False)
        base_path = str(effective_codex.get('baseInstructions', '') or '').strip()
        if base_path:
            raw = Path(base_path)
            if raw.is_absolute():
                if not raw.exists() or raw.is_dir():
                    raise DawnImError(f'baseInstructions file is invalid: {raw}')
            elif '..' in raw.parts:
                raise DawnImError(f'baseInstructions cannot escape the avatar directory: {base_path}')
        dev_path = str(effective_codex.get('developerInstructions', '') or '').strip()
        if dev_path:
            raw = Path(dev_path)
            if raw.is_absolute():
                if not raw.exists() or raw.is_dir():
                    raise DawnImError(f'developerInstructions file is invalid: {raw}')
            elif '..' in raw.parts:
                raise DawnImError(f'developerInstructions cannot escape the avatar directory: {dev_path}')
        model = str(effective_codex.get('model', '') or '').strip()
        provider = str(effective_codex.get('modelProvider', '') or '').strip()
        if model or provider:
            _validate_and_normalize_model_selection(model, provider)
    except DawnImError as err:
        message = str(err)
        warnings.append(message)
        if 'registry is unavailable' in message:
            unknown = True
    if warnings and not unknown:
        return 'invalid', warnings
    if warnings and unknown:
        return 'unknown', warnings
    return 'valid', []


def _find_avatars_for_group(group_folder: str) -> list[str]:
    matches: list[str] = []
    for avatar_id in _list_avatar_ids():
        cfg_path = _avatar_config_path(avatar_id)
        if not cfg_path.exists():
            continue
        cfg = _read_json(cfg_path, {})
        groups = cfg.get('groups') if isinstance(cfg.get('groups'), list) else []
        group_configs = cfg.get('groupConfigs') if isinstance(cfg.get('groupConfigs'), dict) else {}
        if group_folder in {str(g).strip() for g in groups if str(g).strip()} or group_folder in group_configs:
            matches.append(avatar_id)
    return matches


def _resolve_codex_target(args: argparse.Namespace, require_group: bool = False) -> tuple[str, str]:
    force_current = bool(getattr(args, 'current', False))
    explicit_avatar = str(getattr(args, 'avatar_id', '') or '').strip()
    explicit_group = str(getattr(args, 'group', '') or '').strip()
    current_ctx = _resolve_current_context(getattr(args, 'thread_id', '')) if force_current else None

    current_avatar = str((current_ctx or {}).get('avatarId') or '').strip()
    current_group = str((current_ctx or {}).get('groupFolder') or '').strip()

    if force_current:
        if explicit_avatar and current_avatar and explicit_avatar != current_avatar:
            raise DawnImError(
                f"--current resolved avatar '{current_avatar}', but --avatar-id specified '{explicit_avatar}'"
            )
        if explicit_group and current_group and explicit_group != current_group:
            raise DawnImError(
                f"--current resolved group '{current_group}', but --group specified '{explicit_group}'"
            )

    avatar_id = explicit_avatar or current_avatar
    group = explicit_group or current_group

    if group and not avatar_id:
        matches = _find_avatars_for_group(group)
        if not matches:
            raise DawnImError(f"group '{group}' is not bound to any avatar")
        if len(matches) > 1:
            raise DawnImError(
                f"group '{group}' is bound to multiple avatars: {', '.join(matches)}. Pass --avatar-id."
            )
        avatar_id = matches[0]

    if not avatar_id:
        avatar_id = _resolve_avatar_id(args)

    if require_group and not group:
        raise DawnImError('group is required for group scope, or use --current inside a Dawn IM conversation')

    if group:
        cfg = _read_json(_avatar_config_path(avatar_id), {})
        groups = cfg.get('groups') if isinstance(cfg.get('groups'), list) else []
        group_configs = cfg.get('groupConfigs') if isinstance(cfg.get('groupConfigs'), dict) else {}
        normalized_groups = {str(item).strip() for item in groups if str(item).strip()}
        if group not in normalized_groups and group not in group_configs:
            raise DawnImError(f"group '{group}' is not bound to avatar '{avatar_id}'")

    return avatar_id, group


def _compact_group_entry(config: dict[str, Any], group: str) -> None:
    group_configs = config.get('groupConfigs')
    if not isinstance(group_configs, dict):
        return

    entry = group_configs.get(group)
    if isinstance(entry, dict):
        if isinstance(entry.get('codex'), dict) and not entry['codex']:
            entry.pop('codex', None)
        if not entry:
            group_configs.pop(group, None)

    if not group_configs:
        config.pop('groupConfigs', None)


def cmd_codex_config_read(args: argparse.Namespace) -> dict[str, Any]:
    """Read codex config for avatar defaults, group overrides, or effective merge."""
    avatar_id, group = _resolve_codex_target(args, require_group=False)
    scope = str(getattr(args, 'scope', '') or '').strip() or ('effective' if group else 'default')
    if scope not in CODEX_READ_SCOPES:
        raise DawnImError(f"Invalid scope '{scope}'. Must be one of: {', '.join(CODEX_READ_SCOPES)}")
    if scope == 'group' and not group:
        raise DawnImError('group scope requires --group or --current')

    views = _build_codex_views(avatar_id, group)
    default_codex = views['defaultCodex']
    group_codex = views['groupCodex']
    effective_codex = views['effectiveCodex']

    if scope == 'default':
        codex = default_codex
    elif scope == 'group':
        codex = group_codex
    else:
        codex = effective_codex

    config_validity, validation_warnings = _assess_effective_codex_validity(avatar_id, effective_codex)
    return {
        'ok': True,
        'avatarId': avatar_id,
        'group': group or None,
        'scope': scope,
        'codex': codex,
        'defaultCodex': default_codex,
        'groupCodex': group_codex,
        'effectiveCodex': effective_codex,
        'configValidity': config_validity,
        'validationWarnings': validation_warnings,
    }


def cmd_codex_config_set(args: argparse.Namespace) -> dict[str, Any]:
    """Set avatar default codex config or group-level codex overrides."""
    requested_scope = str(getattr(args, 'scope', '') or '').strip()
    scope = requested_scope or (
        'group' if bool(getattr(args, 'current', False) or str(getattr(args, 'group', '') or '').strip()) else 'default'
    )
    if scope not in CODEX_SET_SCOPES:
        raise DawnImError(f"Invalid scope '{scope}'. Must be one of: {', '.join(CODEX_SET_SCOPES)}")

    avatar_id, group = _resolve_codex_target(args, require_group=(scope == 'group'))
    config_path = _avatar_config_path(avatar_id)
    config = _read_json(config_path, {})
    views = _build_codex_views(avatar_id, group)
    default_codex = dict(views['defaultCodex'])
    group_codex = dict(views['groupCodex'])
    avatar_effective_codex = dict(views['avatarEffectiveCodex'])
    changes: dict[str, Any] = {}
    warnings: list[str] = []

    submitted_codex = {
        field: getattr(args, arg_name, None)
        for field, arg_name in CODEX_CONFIG_FIELD_TO_ARG.items()
        if getattr(args, arg_name, None) is not None
    }
    unset_fields = [str(field or '').strip() for field in getattr(args, 'unset', []) or [] if str(field or '').strip()]
    reset_group_override = bool(getattr(args, 'reset_group_override', False))

    if scope == 'default':
        if unset_fields:
            raise DawnImError('--unset is only supported with --scope group')
        if reset_group_override:
            raise DawnImError('--reset-group-override is only supported with --scope group')

        candidate_codex = dict(default_codex)
        if 'approvalPolicy' in submitted_codex:
            candidate_codex['approvalPolicy'] = _normalize_approval_policy_value(submitted_codex['approvalPolicy'], allow_empty=False)
        if 'sandboxPolicy' in submitted_codex:
            candidate_codex['sandboxPolicy'] = _normalize_sandbox_policy_value(submitted_codex['sandboxPolicy'], allow_empty=False)
        if 'workingDirectory' in submitted_codex:
            candidate_codex['workingDirectory'] = _normalize_working_directory_value(submitted_codex['workingDirectory'], allow_empty=False)
        if 'baseInstructions' in submitted_codex:
            candidate_codex['baseInstructions'] = _normalize_prompt_path_value(avatar_id, 'baseInstructions', submitted_codex['baseInstructions'])
        if 'developerInstructions' in submitted_codex:
            candidate_codex['developerInstructions'] = _normalize_prompt_path_value(avatar_id, 'developerInstructions', submitted_codex['developerInstructions'])
        if 'model' in submitted_codex:
            candidate_codex['model'] = str(submitted_codex['model'] or '').strip()
        if 'modelProvider' in submitted_codex:
            candidate_codex['modelProvider'] = str(submitted_codex['modelProvider'] or '').strip()

        if 'model' in submitted_codex or 'modelProvider' in submitted_codex:
            submitted_model = 'model' in submitted_codex
            submitted_provider = 'modelProvider' in submitted_codex
            if submitted_model and not submitted_provider:
                canonical_model, canonical_provider = _validate_and_normalize_model_selection(
                    candidate_codex.get('model', ''),
                    '',
                )
                candidate_codex['model'] = canonical_model
                candidate_codex['modelProvider'] = canonical_provider
            elif submitted_provider and not submitted_model:
                _, canonical_provider = _validate_and_normalize_model_selection(
                    '',
                    candidate_codex.get('modelProvider', ''),
                )
                candidate_codex['modelProvider'] = canonical_provider
            else:
                canonical_model, canonical_provider = _validate_and_normalize_model_selection(
                    candidate_codex.get('model', ''),
                    candidate_codex.get('modelProvider', ''),
                )
                candidate_codex['model'] = canonical_model
                candidate_codex['modelProvider'] = canonical_provider

        config['codex'] = candidate_codex
        for field in CODEX_CONFIG_FIELD_TO_ARG:
            if default_codex.get(field) != candidate_codex.get(field):
                changes[field] = candidate_codex.get(field)
    else:
        if getattr(args, 'base_instructions', None) is not None or getattr(args, 'developer_instructions', None) is not None:
            raise DawnImError('group scope only supports model, modelProvider, approvalPolicy, sandboxPolicy, workingDirectory')

        invalid_unsets = [field for field in unset_fields if field not in GROUP_CODEX_FIELDS]
        if invalid_unsets:
            raise DawnImError(
                f"--unset only supports: {', '.join(GROUP_CODEX_FIELDS)}. Invalid: {', '.join(invalid_unsets)}"
            )

        group_configs = config.get('groupConfigs')
        if not isinstance(group_configs, dict):
            group_configs = {}
            config['groupConfigs'] = group_configs
        entry = group_configs.get(group)
        if not isinstance(entry, dict):
            entry = {}
            group_configs[group] = entry

        candidate_group_codex = {} if reset_group_override else dict(group_codex)
        if 'approvalPolicy' in submitted_codex:
            normalized = _normalize_approval_policy_value(submitted_codex['approvalPolicy'], allow_empty=True)
            if normalized is None:
                candidate_group_codex.pop('approvalPolicy', None)
            else:
                candidate_group_codex['approvalPolicy'] = normalized
        if 'sandboxPolicy' in submitted_codex:
            normalized = _normalize_sandbox_policy_value(submitted_codex['sandboxPolicy'], allow_empty=True)
            if normalized is None:
                candidate_group_codex.pop('sandboxPolicy', None)
            else:
                candidate_group_codex['sandboxPolicy'] = normalized
        if 'workingDirectory' in submitted_codex:
            normalized = _normalize_working_directory_value(submitted_codex['workingDirectory'], allow_empty=True)
            if normalized is None:
                candidate_group_codex.pop('workingDirectory', None)
            else:
                candidate_group_codex['workingDirectory'] = normalized
        if 'model' in submitted_codex:
            normalized_model = str(submitted_codex['model'] or '').strip()
            if normalized_model:
                candidate_group_codex['model'] = normalized_model
            else:
                candidate_group_codex.pop('model', None)
        if 'modelProvider' in submitted_codex:
            normalized_provider = str(submitted_codex['modelProvider'] or '').strip()
            if normalized_provider:
                candidate_group_codex['modelProvider'] = normalized_provider
            else:
                candidate_group_codex.pop('modelProvider', None)

        for field in unset_fields:
            candidate_group_codex.pop(field, None)

        if (
            'model' in submitted_codex
            or 'modelProvider' in submitted_codex
            or 'model' in unset_fields
            or 'modelProvider' in unset_fields
            or reset_group_override
        ):
            explicit_model = str(candidate_group_codex.get('model', '') or '').strip()
            explicit_provider = str(candidate_group_codex.get('modelProvider', '') or '').strip()
            if explicit_model:
                canonical_model, canonical_provider = _validate_and_normalize_model_selection(
                    explicit_model,
                    explicit_provider,
                )
                candidate_group_codex['model'] = canonical_model
                candidate_group_codex['modelProvider'] = canonical_provider
            elif 'modelProvider' in candidate_group_codex:
                _, canonical_provider = _validate_and_normalize_model_selection('', explicit_provider)
                candidate_group_codex['modelProvider'] = canonical_provider

        if candidate_group_codex:
            entry['codex'] = candidate_group_codex
        else:
            entry.pop('codex', None)
        _compact_group_entry(config, group)

        for field in GROUP_CODEX_FIELDS:
            before = group_codex.get(field)
            after = candidate_group_codex.get(field)
            if before != after:
                changes[field] = after
        if reset_group_override and group_codex:
            changes['resetGroupOverride'] = True

    if not changes:
        raise DawnImError('No config changes requested.')

    normalized = any(
        field not in submitted_codex or submitted_codex.get(field) != value
        for field, value in changes.items()
        if field != 'resetGroupOverride'
    )

    _write_json_atomic(config_path, config)
    read_scope = 'effective' if scope == 'group' else 'default'
    current = cmd_codex_config_read(SimpleNamespace(
        avatar_id=avatar_id,
        group=group,
        current=False,
        thread_id='',
        scope=read_scope,
    ))
    target_label = f"{avatar_id}/{group}" if scope == 'group' and group else avatar_id
    return {
        'ok': True,
        'avatarId': avatar_id,
        'group': group or None,
        'scope': scope,
        'changes': changes,
        'normalized': normalized,
        'warnings': warnings,
        'submittedCodex': submitted_codex,
        'appliedCodex': current['codex'],
        'codex': current['codex'],
        'defaultCodex': current['defaultCodex'],
        'groupCodex': current['groupCodex'],
        'effectiveCodex': current['effectiveCodex'],
        'configValidity': current.get('configValidity', 'unknown'),
        'validationWarnings': current.get('validationWarnings', []),
        'message': f"Updated {len(changes)} field(s) for {target_label}",
    }


def cmd_mcp_call(args: argparse.Namespace) -> dict[str, Any]:
    try:
        raw = json.loads(args.args_json or '{}')
    except Exception as err:
        raise DawnImError(f'args-json parse failed: {err}')
    if not isinstance(raw, dict):
        raise DawnImError('args-json must decode to a JSON object')

    tool = str(args.tool or '').strip()
    if not tool:
        raise DawnImError('tool is required')

    timeout_ms = int(raw.get('timeout_ms', args.timeout_ms))

    def ns(**kwargs: Any) -> argparse.Namespace:
        payload = {'timeout_ms': timeout_ms, **kwargs}
        return SimpleNamespace(**payload)

    if tool == 'dawn_im_send':
        return cmd_im_send(ns(
            im_type=raw.get('im_type', ''),
            channel=raw.get('channel', ''),
            text=raw.get('text', ''),
            reply_to=raw.get('reply_to', None),
            avatar_id=raw.get('avatar_id', raw.get('avatarId', '')),
        ))
    if tool == 'dawn_im_react':
        return cmd_im_react(ns(
            im_type=raw.get('im_type', ''),
            channel=raw.get('channel', ''),
            message_id=raw.get('message_id', ''),
            emoji=raw.get('emoji', ''),
        ))
    if tool == 'dawn_im_status':
        return cmd_im_status(ns(im_type=raw.get('im_type', None), timeout_ms=timeout_ms))
    if tool == 'dawn_im_list_channels':
        return cmd_im_list_channels(ns(im_type=raw.get('im_type', None), timeout_ms=timeout_ms))
    if tool == 'dawn_im_create_channel':
        return cmd_im_create_channel(ns(
            im_type=raw.get('im_type', ''),
            name=raw.get('name', ''),
            avatar_id=raw.get('avatar_id', raw.get('avatarId', '')),
            members=raw.get('members', []) or [],
            source_group=raw.get('source_group', raw.get('sourceGroup', '')),
            current=bool(raw.get('current', False)),
            thread_id=raw.get('thread_id', raw.get('threadId', '')),
            prefer_current_avatar=bool(raw.get('prefer_current_avatar', raw.get('preferCurrentAvatar', True))),
            allow_private_chat=_coerce_bool(raw.get('allow_private_chat', raw.get('allowPrivateChat', True)), default=True),
            timeout_ms=timeout_ms,
        ))
    if tool == 'dawn_im_delete_channel':
        return cmd_im_delete_channel(ns(
            im_type=raw.get('im_type', ''),
            folder=raw.get('folder', ''),
            avatar_id=raw.get('avatar_id', ''),
            leave_service=bool(raw.get('leave_service', False)),
            timeout_ms=timeout_ms,
        ))
    if tool == 'dawn_im_register_channel':
        return cmd_im_register_channel(ns(
            im_type=raw.get('im_type', ''),
            channel_id=raw.get('channel_id', ''),
            avatar_id=raw.get('avatar_id', ''),
            display_name=raw.get('display_name', ''),
            timeout_ms=timeout_ms,
        ))
    if tool == 'dawn_im_action':
        return cmd_im_action(ns(
            im_type=raw.get('im_type', ''),
            action=raw.get('action', ''),
            channel=raw.get('channel', ''),
            params_json=json.dumps(raw.get('params', {}), ensure_ascii=False),
            timeout_ms=timeout_ms,
        ))
    if tool == 'dawn_context_current':
        return cmd_context_current(ns(
            thread_id=raw.get('thread_id', raw.get('threadId', '')),
        ))
    if tool == 'dawn_background_dispatch':
        items = raw.get('items', raw.get('items_json', []))
        return cmd_background_dispatch(ns(
            items_json=json.dumps(items, ensure_ascii=False),
            job_id=raw.get('job_id', raw.get('jobId', '')),
            timeout_ms=timeout_ms,
        ))
    if tool == 'dawn_background_status':
        return cmd_background_status(ns(
            job_id=raw.get('job_id', raw.get('jobId', '')),
            timeout_ms=timeout_ms,
        ))
    if tool == 'dawn_codex_new_chat':
        return cmd_codex_new_chat(ns(
            im_type=raw.get('im_type', None),
            group=raw.get('group', ''),
            avatar_id=raw.get('avatar_id', ''),
            current=bool(raw.get('current', False)),
            thread_id=raw.get('thread_id', raw.get('threadId', '')),
            timeout_ms=timeout_ms,
        ))
    if tool == 'dawn_codex_fork_chat':
        return cmd_codex_fork_chat(ns(
            im_type=raw.get('im_type', None),
            name=raw.get('name', ''),
            source_group=raw.get('source_group', ''),
            avatar_id=raw.get('avatar_id', ''),
            source_thread_id=raw.get('source_thread_id', ''),
            current=bool(raw.get('current', False)),
            thread_id=raw.get('thread_id', raw.get('threadId', '')),
            timeout_ms=timeout_ms,
        ))

    if tool == 'dawn_service_restart':
        return cmd_service_restart(ns(
            service=raw.get('service', 'all'),
            include_settings=bool(raw.get('include_settings', False)),
            timeout_ms=timeout_ms,
        ))

    if tool == 'dawn_codex_config_read':
        return cmd_codex_config_read(ns(
            avatar_id=raw.get('avatar_id', raw.get('avatarId', '')),
            group=raw.get('group', raw.get('group_folder', raw.get('groupFolder', ''))),
            current=bool(raw.get('current', False)),
            thread_id=raw.get('thread_id', raw.get('threadId', '')),
            scope=raw.get('scope', ''),
        ))

    if tool == 'dawn_codex_config_set':
        unset_fields = raw.get('unset', [])
        if isinstance(unset_fields, str):
            unset_fields = [unset_fields]
        return cmd_codex_config_set(ns(
            avatar_id=raw.get('avatar_id', raw.get('avatarId', '')),
            group=raw.get('group', raw.get('group_folder', raw.get('groupFolder', ''))),
            current=bool(raw.get('current', False)),
            thread_id=raw.get('thread_id', raw.get('threadId', '')),
            scope=raw.get('scope', ''),
            model=raw.get('model'),
            model_provider=raw.get('model_provider', raw.get('modelProvider')),
            approval_policy=raw.get('approval_policy', raw.get('approvalPolicy')),
            sandbox_policy=raw.get('sandbox_policy', raw.get('sandboxPolicy')),
            working_directory=raw.get('working_directory', raw.get('workingDirectory')),
            base_instructions=raw.get('base_instructions', raw.get('baseInstructions')),
            developer_instructions=raw.get('developer_instructions', raw.get('developerInstructions')),
            unset=unset_fields,
            reset_group_override=bool(raw.get('reset_group_override', raw.get('resetGroupOverride', False))),
        ))

    raise DawnImError(f'unsupported tool: {tool}')


def _build_parser() -> argparse.ArgumentParser:
    p = argparse.ArgumentParser(description='Dawn IM management via direct IPC (no MCP)')
    sp = p.add_subparsers(dest='command', required=True)

    def add_common(cmd: argparse.ArgumentParser, allow_empty_channel: bool = True) -> None:
        cmd.add_argument('--timeout-ms', type=int, default=15000)
        cmd.add_argument('--json', action='store_true')
        if not allow_empty_channel:
            cmd.add_argument('--channel', required=True)

    c = sp.add_parser('im-status')
    add_common(c)
    c.add_argument('--im-type', choices=IM_TYPES)

    c = sp.add_parser('im-list-channels')
    add_common(c)
    c.add_argument('--im-type', choices=IM_TYPES)

    c = sp.add_parser('im-send')
    add_common(c)
    c.add_argument('--im-type', choices=IM_TYPES, required=True)
    c.add_argument('--channel', required=True)
    c.add_argument('--text', required=True)
    c.add_argument('--reply-to')
    c.add_argument('--avatar-id', default='')

    c = sp.add_parser('im-react')
    add_common(c)
    c.add_argument('--im-type', choices=IM_TYPES, required=True)
    c.add_argument('--channel', required=True)
    c.add_argument('--message-id', required=True)
    c.add_argument('--emoji', required=True)

    c = sp.add_parser('im-register-channel')
    add_common(c)
    c.add_argument('--im-type', choices=IM_TYPES, required=True)
    c.add_argument('--channel-id', required=True)
    c.add_argument('--avatar-id', required=True)
    c.add_argument('--display-name', default='')

    c = sp.add_parser('im-create-channel')
    add_common(c)
    c.add_argument('--im-type', choices=IM_TYPES, required=True)
    c.add_argument('--name', required=True)
    c.add_argument('--avatar-id', default='')
    c.add_argument('--members', nargs='*', default=[])
    c.add_argument('--source-group', default='')
    c.add_argument('--current', action='store_true', help='Use current Dawn IM context defaults when available')
    c.add_argument('--thread-id', default='', help='Explicit thread id (default: CODEX_THREAD_ID)')
    c.add_argument('--prefer-current-avatar', action='store_true', help='Prefer current context avatar over implicit defaults')
    c.add_argument(
        '--allow-private-chat',
        action='store_true',
        default=None,
        help='Allow Feishu create to fall back to a private chat when only one member is available',
    )

    c = sp.add_parser('im-delete-channel')
    add_common(c)
    c.add_argument('--im-type', choices=IM_TYPES, required=True)
    c.add_argument('--folder', required=True)
    c.add_argument('--avatar-id', default='')
    c.add_argument('--leave-service', action='store_true')

    c = sp.add_parser('im-action')
    add_common(c)
    c.add_argument('--im-type', choices=IM_TYPES, required=True)
    c.add_argument('--action', required=True)
    c.add_argument('--channel', default='')
    c.add_argument('--params-json', default='{}')

    c = sp.add_parser('context-current')
    add_common(c)
    c.add_argument('--thread-id', default='', help='Explicit thread id (default: CODEX_THREAD_ID)')

    c = sp.add_parser('background-dispatch')
    add_common(c)
    c.set_defaults(timeout_ms=2000)
    c.add_argument('--items-json', required=True, help='JSON array of items or object with items[]')
    c.add_argument('--job-id', default='', help='Optional stable job id')

    c = sp.add_parser('background-status')
    add_common(c)
    c.set_defaults(timeout_ms=2000)
    c.add_argument('--job-id', required=True)

    c = sp.add_parser('codex-new-chat')
    add_common(c)
    c.add_argument('--im-type', choices=IM_TYPES)
    c.add_argument('--group', default='')
    c.add_argument('--avatar-id', default='')
    c.add_argument('--current', action='store_true', help='Require and use the current Dawn IM context')
    c.add_argument('--thread-id', default='', help='Explicit thread id (default: CODEX_THREAD_ID)')

    c = sp.add_parser('codex-fork-chat')
    add_common(c)
    c.add_argument('--im-type', choices=IM_TYPES)
    c.add_argument('--name', required=True)
    c.add_argument('--source-group', default='')
    c.add_argument('--avatar-id', default='')
    c.add_argument('--source-thread-id', default='')
    c.add_argument('--current', action='store_true', help='Require and use the current Dawn IM context')
    c.add_argument('--thread-id', default='', help='Explicit thread id (default: CODEX_THREAD_ID)')

    c = sp.add_parser('service-restart')
    c.add_argument('--service', choices=[*SERVICE_NAMES, 'all'], default='all')
    c.add_argument('--include-settings', action='store_true', help='Reserved; restarting Dawn Settings itself is not supported here.')
    c.add_argument('--timeout-ms', type=int, default=30000)
    c.add_argument('--json', action='store_true')

    c = sp.add_parser('codex-config-read')
    add_common(c)
    c.add_argument('--avatar-id', default='', help='Avatar ID (default: active avatar)')
    c.add_argument('--group', default='', help='Target group folder')
    c.add_argument('--current', action='store_true', help='Use current Dawn IM context')
    c.add_argument('--thread-id', default='', help='Explicit thread id (default: CODEX_THREAD_ID)')
    c.add_argument('--scope', choices=CODEX_READ_SCOPES, default=None, help='default, group, effective')

    c = sp.add_parser('codex-config-set')
    add_common(c)
    c.add_argument('--avatar-id', default='', help='Avatar ID (default: active avatar)')
    c.add_argument('--group', default='', help='Target group folder')
    c.add_argument('--current', action='store_true', help='Use current Dawn IM context')
    c.add_argument('--thread-id', default='', help='Explicit thread id (default: CODEX_THREAD_ID)')
    c.add_argument('--scope', choices=CODEX_SET_SCOPES, default=None, help='default or group')
    c.add_argument('--model', default=None, help='Model name (e.g. claude-opus-4-6)')
    c.add_argument('--model-provider', default=None, help='Model provider (openai, openrouter, minimax)')
    c.add_argument('--approval-policy', default=None, help='never, on-request, on-failure, untrusted')
    c.add_argument('--sandbox-policy', default=None, help='workspace-write, danger-full-access, read-only, external-sandbox')
    c.add_argument('--working-directory', default=None, help='Working directory path')
    c.add_argument('--base-instructions', default=None, help='Base instructions file path')
    c.add_argument('--developer-instructions', default=None, help='Developer instructions file path')
    c.add_argument('--unset', action='append', default=[], help='Clear a group override field; repeatable')
    c.add_argument('--reset-group-override', action='store_true', help='Clear all codex overrides for the target group')

    c = sp.add_parser('mcp-call')
    c.add_argument('--tool', required=True)
    c.add_argument('--args-json', default='{}')
    c.add_argument('--timeout-ms', type=int, default=15000)
    c.add_argument('--json', action='store_true')

    return p


def _print_result(payload: dict[str, Any], as_json: bool) -> None:
    if as_json:
        print(json.dumps(payload, indent=2, ensure_ascii=False))
        return
    print(json.dumps(payload, ensure_ascii=False))


def main() -> int:
    parser = _build_parser()
    args = parser.parse_args()

    try:
        if args.command in {'im-create-channel', 'im-delete-channel', 'im-send', 'im-react', 'im-register-channel', 'im-action'}:
            _require_im_type(args.im_type)
        elif args.command in {'im-status', 'im-list-channels'} and args.im_type:
            _require_im_type(args.im_type)

        handlers = {
            'im-status': cmd_im_status,
            'im-list-channels': cmd_im_list_channels,
            'im-send': cmd_im_send,
            'im-react': cmd_im_react,
            'im-register-channel': cmd_im_register_channel,
            'im-create-channel': cmd_im_create_channel,
            'im-delete-channel': cmd_im_delete_channel,
            'im-action': cmd_im_action,
            'context-current': cmd_context_current,
            'background-dispatch': cmd_background_dispatch,
            'background-status': cmd_background_status,
            'codex-new-chat': cmd_codex_new_chat,
            'codex-fork-chat': cmd_codex_fork_chat,
            'codex-config-read': cmd_codex_config_read,
            'codex-config-set': cmd_codex_config_set,
            'service-restart': cmd_service_restart,
            'mcp-call': cmd_mcp_call,
        }
        result = handlers[args.command](args)
        _print_result(result, bool(getattr(args, 'json', False)))
        return 0
    except DawnImError as err:
        _print_result({'ok': False, 'error': str(err)}, True)
        return 1
    except Exception as err:
        _print_result({'ok': False, 'error': f'unexpected_error: {err}'}, True)
        return 1


if __name__ == '__main__':
    raise SystemExit(main())
