"""离线宿主候选协议。非生产执行适配器；永不授予来源或执行权限。

仅收集有界 USTAR 文件流；不解包、不写工作区。候选不可变仅指进程内
值对象，不证明持久存储封存或执行控制权。POSIX 进程组清理不覆盖主动
setsid 逃逸，因此不能证明所有后代退出。正常交付和 hooks 路径不接线。
"""
from dataclasses import dataclass
import hashlib
import json
import math
import os
import selectors
import signal
import subprocess
import sys
import time
import unicodedata
from collections.abc import Mapping, Sequence


class CandidateError(ValueError):
    pass


@dataclass(frozen=True)
class Limits:
    entries: int = 64
    depth: int = 16
    file_bytes: int = 2 * 1024 * 1024
    total_bytes: int = 8 * 1024 * 1024
    archive_bytes: int = 10 * 1024 * 1024

    def __post_init__(self):
        caps = (64, 16, 2 * 1024 * 1024, 8 * 1024 * 1024, 10 * 1024 * 1024)
        if any(type(value) is not int or value <= 0 or value > cap for value, cap in zip(vars(self).values(), caps)):
            raise CandidateError('INVALID_LIMIT')


@dataclass(frozen=True)
class File:
    data: bytes
    mode: int

    def __post_init__(self):
        if type(self.data) is not bytes or self.mode not in (0o644, 0o755):
            raise CandidateError('INVALID_FILE')


@dataclass(frozen=True)
class Candidate:
    files: tuple[tuple[str, File], ...]
    directories: tuple[str, ...]
    digest: str

    def __post_init__(self):
        _validate_candidate(self)

    @property
    def admitted(self):
        return False


@dataclass(frozen=True)
class Plan:
    changes: tuple[str, ...]
    conflicts: tuple[str, ...]
    candidate_digest: str

    @property
    def apply_allowed(self):
        return False


def _path(name, depth):
    if (not name or len(name.encode('utf-8')) > 1024 or '\\' in name or ':' in name
            or unicodedata.normalize('NFC', name) != name
            or any(ord(c) < 32 or ord(c) == 127 for c in name)):
        raise CandidateError('INVALID_PATH')
    parts = name.split('/')
    if len(parts) > depth or any(p in ('', '.', '..') for p in parts):
        raise CandidateError('INVALID_PATH_DEPTH_OR_ALIAS')
    return name


def _text(raw):
    first, separator, tail = raw.partition(b'\0')
    if separator and tail.strip(b'\0'):
        raise CandidateError('INVALID_HEADER_TEXT')
    return first.decode('utf-8', errors='strict')


def _octal(raw):
    value = raw.strip(b'\0 ')
    if not value or any(c not in b'01234567' for c in value):
        raise CandidateError('INVALID_HEADER_NUMBER')
    return int(value, 8)


def _digest(files, directories):
    manifest = {'files': [(n, f.mode, len(f.data), hashlib.sha256(f.data).hexdigest()) for n, f in files], 'directories': list(directories)}
    return hashlib.sha256(json.dumps(manifest, sort_keys=True, separators=(',', ':')).encode()).hexdigest()


def _validate_snapshot(snapshot):
    if len(snapshot) > 64:
        raise CandidateError('SNAPSHOT_ENTRY_BUDGET')
    total, namespace = 0, {}
    for path, value in snapshot.items():
        if type(path) is not str or type(value) is not File:
            raise CandidateError('INVALID_SNAPSHOT')
        _path(path, 16)
        if len(value.data) > 2 * 1024 * 1024:
            raise CandidateError('SNAPSHOT_FILE_BUDGET')
        total += len(value.data)
        if total > 8 * 1024 * 1024:
            raise CandidateError('SNAPSHOT_TOTAL_BUDGET')
        for i in range(1, len(path.split('/')) + 1):
            part = '/'.join(path.split('/')[:i])
            if namespace.setdefault(part.casefold(), part) != part:
                raise CandidateError('SNAPSHOT_PATH_ALIAS')
    for path in snapshot:
        if any(path.startswith(other + '/') for other in snapshot if other != path):
            raise CandidateError('SNAPSHOT_FILE_DIRECTORY_COLLISION')


def _validate_candidate(candidate):
    if type(candidate.files) is not tuple or type(candidate.directories) is not tuple:
        raise CandidateError('MUTABLE_CANDIDATE')
    if len(candidate.files) + len(candidate.directories) > 64:
        raise CandidateError('CANDIDATE_ENTRY_BUDGET')
    if any(type(item) is not tuple or len(item) != 2 for item in candidate.files):
        raise CandidateError('INVALID_CANDIDATE')
    files = dict(candidate.files)
    _validate_snapshot(files)
    if len(files) != len(candidate.files) or candidate.files != tuple(sorted(files.items())):
        raise CandidateError('NONCANONICAL_CANDIDATE')
    dirs = candidate.directories
    if any(type(d) is not str for d in dirs) or dirs != tuple(sorted(set(dirs))):
        raise CandidateError('NONCANONICAL_DIRECTORIES')
    namespace = {}
    for path in (*files.keys(), *dirs):
        _path(path, 16)
        for i in range(1, len(path.split('/')) + 1):
            part = '/'.join(path.split('/')[:i])
            if namespace.setdefault(part.casefold(), part) != part:
                raise CandidateError('CANDIDATE_PATH_ALIAS')
    if any(d == f or d.startswith(f + '/') for d in dirs for f in files):
        raise CandidateError('CANDIDATE_FILE_DIRECTORY_COLLISION')
    if candidate.digest != _digest(candidate.files, dirs):
        raise CandidateError('CANDIDATE_DIGEST_MISMATCH')


def collect_archive(raw: bytes, limits: Limits = Limits()) -> Candidate:
    """Strict USTAR only: no PAX/GNU extensions, compression, links or devices."""
    if type(raw) is not bytes or len(raw) > limits.archive_bytes:
        raise CandidateError('ARCHIVE_BUDGET')
    files, dirs, seen, namespace = {}, [], {}, {}
    offset = total = count = 0
    try:
        while True:
            header = raw[offset:offset + 512]
            if len(header) != 512:
                raise CandidateError('TRUNCATED_ARCHIVE')
            offset += 512
            if header == bytes(512):
                if len(raw) < offset + 512 or raw[offset:offset + 512] != bytes(512) or any(raw[offset:]):
                    raise CandidateError('INVALID_ARCHIVE_END')
                break
            count += 1
            if count > limits.entries:
                raise CandidateError('ENTRY_BUDGET')
            if header[257:265] != b'ustar\00000':
                raise CandidateError('UNSUPPORTED_ARCHIVE_FORMAT')
            if _octal(header[148:156]) != sum(header[:148]) + 8 * 32 + sum(header[156:]):
                raise CandidateError('HEADER_CHECKSUM')
            kind = header[156:157]
            if kind not in (b'0', b'\0', b'5') or _text(header[157:257]):
                raise CandidateError('UNSUPPORTED_ENTRY_TYPE')
            name = _text(header[:100])
            prefix = _text(header[345:500])
            if prefix:
                name = prefix + '/' + name
            if kind == b'5' and name.endswith('/'):
                name = name[:-1]
            name = _path(name, limits.depth)
            for i in range(1, len(name.split('/')) + 1):
                part = '/'.join(name.split('/')[:i])
                prior_spelling = namespace.setdefault(part.casefold(), part)
                if prior_spelling != part:
                    raise CandidateError('PATH_COMPONENT_ALIAS')
            folded = name.casefold()
            if folded in seen:
                raise CandidateError('DUPLICATE_PATH_ALIAS')
            for prior, is_dir in seen.items():
                if (folded.startswith(prior + '/') and not is_dir) or (prior.startswith(folded + '/') and kind != b'5'):
                    raise CandidateError('FILE_DIRECTORY_COLLISION')
            seen[folded] = kind == b'5'
            size, mode = _octal(header[124:136]), _octal(header[100:108])
            if mode not in (0o644, 0o755) or (kind == b'5' and (size or mode != 0o755)):
                raise CandidateError('INVALID_MODE_OR_DIRECTORY_SIZE')
            if size > limits.file_bytes or total + size > limits.total_bytes:
                raise CandidateError('CONTENT_BUDGET')
            total += size
            end = offset + ((size + 511) // 512) * 512
            if end > len(raw) or any(raw[offset + size:end]):
                raise CandidateError('TRUNCATED_OR_INVALID_PADDING')
            if kind == b'5':
                dirs.append(name)
            else:
                files[name] = File(raw[offset:offset + size], mode)
            offset = end
    except (UnicodeError, OverflowError) as error:
        raise CandidateError('INVALID_HEADER') from error
    ordered = tuple(sorted(files.items()))
    return Candidate(ordered, tuple(sorted(dirs)), _digest(ordered, tuple(sorted(dirs))))


# Trusted supervisor keeps the group leader alive until host cleanup. Exporter
# cannot inherit the status fd; this is lifecycle plumbing, NOT a sandbox.
_EXPORT_SUPERVISOR = r"""
import os, subprocess, sys
fd = int(sys.argv[1])
try:
    child = subprocess.Popen(sys.argv[2:], stdin=subprocess.DEVNULL, close_fds=True)
    result = 'exit:' + str(child.wait())
except Exception:
    result = 'launch-error'
os.write(fd, result.encode('ascii'))
os.close(fd)
os.close(1)
os.read(0, 1)
"""


def collect_process(command: Sequence[str], *, timeout: float = 5.0,
                    limits: Limits = Limits()) -> Candidate:
    """Bounded trusted exporter; leader remains unreaped and alive for killpg.

    No ownership admission; escaped descendants and denied group cleanup remain
    unproven. Never signal a group after reaping its leader.
    """
    if os.name != 'posix':
        raise CandidateError('UNSUPPORTED_PLATFORM')
    if not math.isfinite(timeout) or timeout <= 0 or not command:
        raise CandidateError('INVALID_TIMEOUT_OR_COMMAND')
    deadline = time.monotonic() + timeout
    read_fd, write_fd = os.pipe()
    process = None
    primary = None
    cleanup = []
    try:
        try:
            process = subprocess.Popen(
                [sys.executable, '-c', _EXPORT_SUPERVISOR, str(write_fd), *command],
                stdout=subprocess.PIPE, stderr=subprocess.DEVNULL,
                stdin=subprocess.PIPE, start_new_session=True, pass_fds=(write_fd,))
        finally:
            os.close(write_fd)
        assert process.stdout is not None
        output, status = bytearray(), bytearray()
        os.set_blocking(process.stdout.fileno(), False)
        os.set_blocking(read_fd, False)
        with selectors.DefaultSelector() as selector:
            selector.register(process.stdout, selectors.EVENT_READ, 'output')
            selector.register(read_fd, selectors.EVENT_READ, 'status')
            while selector.get_map():
                remaining = deadline - time.monotonic()
                if remaining <= 0:
                    raise CandidateError('COLLECT_TIMEOUT')
                for key, _ in selector.select(remaining):
                    target, budget = (output, limits.archive_bytes) if key.data == 'output' else (status, 64)
                    chunk = os.read(key.fd, min(65536, budget + 1 - len(target)))
                    if not chunk:
                        selector.unregister(key.fileobj)
                    else:
                        target.extend(chunk)
                        if len(target) > budget:
                            raise CandidateError('ARCHIVE_BUDGET' if key.data == 'output' else 'EXPORT_STATUS_BUDGET')
        if status == b'launch-error':
            raise CandidateError('EXPORT_LAUNCH_FAILED')
        if not status.startswith(b'exit:') or not status[5:].lstrip(b'-').isdigit():
            raise CandidateError('EXPORT_STATUS_INVALID')
        if status != b'exit:0':
            raise CandidateError('EXPORT_EXIT_NONZERO')
        return collect_archive(bytes(output), limits)
    except BaseException as error:
        primary = error
        raise
    finally:
        # Each cleanup step runs even if previous ones fail. EPERM is never
        # treated as successful group cleanup; preserve original error as primary.
        if process is not None:
            try:
                os.killpg(process.pid, signal.SIGKILL)
            except ProcessLookupError:
                pass
            except BaseException as error:
                cleanup.append('killpg:' + type(error).__name__ + ':' + str(getattr(error, 'errno', None)))
                try:
                    os.kill(process.pid, signal.SIGKILL)
                except ProcessLookupError:
                    pass
                except BaseException as direct_error:
                    cleanup.append('leader-kill:' + type(direct_error).__name__)
            for stream in (process.stdout, process.stdin):
                if stream is not None:
                    try:
                        stream.close()
                    except OSError as error:
                        cleanup.append('close:' + type(error).__name__)
            try:
                process.wait(timeout=1.0)
            except (OSError, subprocess.TimeoutExpired) as error:
                cleanup.append('reap:' + type(error).__name__)
        try:
            os.close(read_fd)
        except OSError as error:
            cleanup.append('status-close:' + type(error).__name__)
        if cleanup:
            detail = 'CLEANUP_UNCONFIRMED ' + ','.join(cleanup)
            if primary is not None:
                primary.add_note(detail)
            else:
                raise CandidateError(detail)


def conflict_plan(candidate: Candidate, baseline: Mapping[str, File],
                  current: Mapping[str, File]) -> Plan:
    """Pure proposal over supplied snapshots; neither authenticates nor writes them."""
    if type(candidate) is not Candidate:
        raise CandidateError('INVALID_CANDIDATE')
    _validate_candidate(candidate)
    # Require finite ordinary maps; arbitrary Mapping implementations may block.
    if type(baseline) is not dict or type(current) is not dict:
        raise CandidateError('UNSUPPORTED_SNAPSHOT_MAPPING')
    _validate_snapshot(baseline)
    _validate_snapshot(current)
    before, now = dict(baseline), dict(current)
    after = dict(candidate.files)
    old_dirs = {'/'.join(p.split('/')[:i]) for p in before for i in range(1, len(p.split('/')))}
    changed = {p for p in before.keys() | after.keys() if before.get(p) != after.get(p)}
    changed.update(p for p in candidate.directories if p not in old_dirs)
    conflicts = set()
    for path in changed:
        if now.get(path) != before.get(path):
            conflicts.add(path)
        for other in now:
            a, b = path.casefold(), other.casefold()
            for left, right in zip(path.split('/'), other.split('/')):
                if left.casefold() != right.casefold():
                    break
                if left != right:
                    conflicts.add(path)
            if (a == b or a.startswith(b + '/') or b.startswith(a + '/')) and (
                    now[other] != before.get(other) or (other != path and a == b)):
                conflicts.add(path)
    return Plan(tuple(sorted(changed)), tuple(sorted(conflicts)), candidate.digest)
