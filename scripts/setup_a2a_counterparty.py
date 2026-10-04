#!/usr/bin/env python3
"""Prepare reviewed, private configuration for existing A2A counterparties.

This helper never activates a Node, issues an invitation, changes a signed
artifact, or opens anonymous admission. The optional A2A bearer and the existing
provider-wide admission invitation remain two separate credentials.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import secrets
import shlex
import shutil
import stat
import sys
import tempfile
from urllib.parse import urlsplit

LIMIT = 64 * 1024
SCHEMA = "froglet/a2a-counterparty-setup/v1"
HASH = re.compile(r"[0-9a-f]{64}\Z")
TOKEN = re.compile(r"[A-Za-z0-9_-]{32,256}\Z")
ADMIN_ENV = ("FROGLET_RUNTIME_AUTH_TOKEN", "FROGLET_PROVIDER_CONTROL_AUTH_TOKEN",
             "FROGLET_CONSUMER_CONTROL_AUTH_TOKEN", "FROGLET_PROVIDER_CONTROL_TOKEN",
             "FROGLET_CONSUMER_CONTROL_TOKEN")
ADMIN_FILES = ("auth.token", "consumerctl.token", "froglet-control.token")


class SetupError(ValueError):
    """Public-safe failure: never include credential-bearing source text."""


def digest(value):
    return hashlib.sha256(value).hexdigest()


def encoded(value):
    return (json.dumps(value, sort_keys=True, indent=2) + "\n").encode()


def private_read(path):
    if not isinstance(path, (str, Path)):
        raise SetupError("credential/configuration file paths must be absolute")
    path = Path(path)
    if not path.is_absolute():
        raise SetupError("credential/configuration file paths must be absolute")
    descriptor = os.open(path, os.O_RDONLY | getattr(os, "O_NOFOLLOW", 0)
                         | getattr(os, "O_NONBLOCK", 0))
    with os.fdopen(descriptor, "rb") as source:
        meta = os.fstat(source.fileno())
        if not stat.S_ISREG(meta.st_mode) or meta.st_size > LIMIT:
            raise SetupError("credential/configuration must be a regular file of at most 64 KiB")
        if os.name == "posix" and (stat.S_IMODE(meta.st_mode) != 0o600
                                  or meta.st_uid != os.geteuid()):
            raise SetupError("credential/configuration must be owned by this user with mode 0600")
        value = source.read(LIMIT + 1)
        if len(value) > LIMIT:
            raise SetupError("credential/configuration grew beyond 64 KiB")
        return value


def parse_private(path):
    value = private_read(path)
    try:
        def unique_pairs(pairs):
            result = {}
            for key, item in pairs:
                if key in result:
                    raise ValueError("duplicate key")
                result[key] = item
            return result
        return json.loads(value, object_pairs_hook=unique_pairs), value
    except (ValueError, UnicodeError):
        raise SetupError("invalid private JSON; credential-bearing source text withheld") from None


def fields(value, required, optional=()):
    if not isinstance(value, dict) or set(value) - set(required) - set(optional) \
            or set(required) - set(value):
        raise SetupError("unexpected or missing setup fields")


def hash_value(value):
    if not isinstance(value, str) or not HASH.fullmatch(value):
        raise SetupError("identities and Offer hashes must be 64 lowercase hexadecimal characters")
    return value


def operator_tokens():
    """Inspect known local operator credentials without copying or printing them."""
    result = {os.environ[name].strip() for name in ADMIN_ENV if name in os.environ}
    paths = set()
    for name in ("FROGLET_RUNTIME_AUTH_TOKEN_PATH", "FROGLET_PROVIDER_CONTROL_TOKEN_PATH",
                 "FROGLET_CONSUMER_CONTROL_TOKEN_PATH", "FROGLET_PROVIDER_AUTH_TOKEN_PATH",
                 "FROGLET_AUTH_TOKEN_PATH"):
        if name in os.environ:
            # Native paths are literal; the optional JS integrations trim and
            # expand '~'. Inspect both forms where they differ.
            paths.add(Path(os.environ[name]))
            paths.add(Path(os.environ[name].strip()).expanduser())
    # Runtime/Node prefer DATA_ROOT; the legacy provider-control client prefers
    # DATA_DIR. Inspect both if present so a mixed environment cannot disguise
    # either surface's operator credential as an invitation.
    roots = [Path(os.environ[name]) for name in ("FROGLET_DATA_ROOT", "FROGLET_DATA_DIR")
             if name in os.environ]
    # The provider-control client falls back to HOME when DATA_DIR is absent,
    # even if the Node/requester uses DATA_ROOT. Conservatively inspect all
    # known defaults in addition to explicit roots.
    roots.extend((Path("data"), Path.home() / ".froglet", Path.home() / ".froglet/data"))
    paths.update(root / "runtime" / name for root in roots for name in ADMIN_FILES)
    for path in paths:
        try:
            descriptor = os.open(path, os.O_RDONLY | getattr(os, "O_NOFOLLOW", 0)
                                 | getattr(os, "O_NONBLOCK", 0))
        except FileNotFoundError:
            continue
        with os.fdopen(descriptor, "rb") as source:
            meta = os.fstat(source.fileno())
            if not stat.S_ISREG(meta.st_mode) or meta.st_size > 1024 \
                    or (os.name == "posix" and meta.st_uid != os.geteuid()):
                raise SetupError("cannot safely inspect configured operator credentials")
            try:
                token = source.read(1025).decode().strip()
            except UnicodeError:
                raise SetupError("configured operator credential is invalid; value withheld") from None
            if token:
                result.add(token)
    return result


def credential(value, invite=False):
    valid = isinstance(value, str) and (TOKEN.fullmatch(value) if not invite else
                                       32 <= len(value) <= 256 and value.isascii()
                                       and all(33 <= ord(char) <= 126 for char in value))
    if not valid:
        raise SetupError("invalid private credential; value withheld")
    if value in operator_tokens():
        raise SetupError("counterparty credentials must be separate from operator/runtime credentials")
    return value


def origin(value, allow_loopback):
    if not isinstance(value, str) or not isinstance(allow_loopback, bool):
        raise SetupError("provider origin and loopback permission have invalid types")
    try:
        parsed = urlsplit(value)
        port = parsed.port
    except ValueError:
        raise SetupError("invalid provider origin; source withheld") from None
    loopback = parsed.hostname in ("127.0.0.1", "::1")
    if not parsed.hostname or parsed.username is not None or parsed.password is not None \
            or parsed.path not in ("", "/") or parsed.query or parsed.fragment \
            or "?" in value or "#" in value or (allow_loopback and not loopback) \
            or (parsed.scheme != "https" and not
                (parsed.scheme == "http" and loopback and allow_loopback)) \
            or port == 0 or any(ord(char) < 33 for char in value):
        raise SetupError("provider URL must be a credential-free HTTPS origin, or explicitly approved literal loopback HTTP")
    try:
        host = parsed.hostname.encode("idna").decode("ascii").lower()
    except UnicodeError:
        raise SetupError("invalid provider hostname; source withheld") from None
    if ":" not in host and not re.fullmatch(r"[a-z0-9.-]+", host):
        raise SetupError("invalid provider hostname; source withheld")
    if ":" in host:
        host = "[" + host + "]"
    default_port = 443 if parsed.scheme == "https" else 80
    suffix = ":" + str(port) if port is not None and port != default_port else ""
    return parsed.scheme + "://" + host + suffix


def offers(value):
    if not isinstance(value, list) or not 1 <= len(value) <= 128:
        raise SetupError("approve 1–128 exact Offer hashes")
    result = [hash_value(item) for item in value]
    if len(set(result)) != len(result):
        raise SetupError("duplicate Offer scopes are not accepted")
    return sorted(result)


def validate_config(value):
    fields(value, (), ("clients", "providers"))
    clients, providers = value.setdefault("clients", []), value.setdefault("providers", [])
    if not isinstance(clients, list) or not isinstance(providers, list) \
            or len(clients) > 128 or len(providers) > 128:
        raise SetupError("A2A configuration supports at most 128 clients and providers")
    tokens, urls = set(), set()
    for client in clients:
        fields(client, ("requester_id", "token", "offer_hashes"))
        hash_value(client["requester_id"])
        offers(client["offer_hashes"])
        token = credential(client["token"])
        if token in tokens:
            raise SetupError("A2A client credentials must be unique")
        tokens.add(token)
    for provider in providers:
        fields(provider, ("provider_url", "token"), ("allow_loopback",))
        credential(provider["token"])
        url = origin(provider["provider_url"], provider.get("allow_loopback", False))
        provider["provider_url"] = url
        if url in urls:
            raise SetupError("A2A provider origins must be unique")
        urls.add(url)
    if len(encoded(value)) > LIMIT:
        raise SetupError("merged A2A configuration exceeds 64 KiB")
    return value


def destination(value):
    if not isinstance(value, str):
        raise SetupError("destination must be an absolute new directory")
    path = Path(value)
    if not path.is_absolute() or ".." in path.parts or path.name in ("", "."):
        raise SetupError("destination must be an absolute new directory")
    for ancestor in [path, *path.parents]:
        if ancestor.is_symlink():
            raise SetupError("destination and its parents must not be symbolic links")
    if path.exists():
        raise SetupError("destination already exists; nothing is overwritten")
    meta = path.parent.stat()
    if not stat.S_ISDIR(meta.st_mode) or (os.name == "posix" and
            (meta.st_uid != os.geteuid() or meta.st_mode & 0o022)):
        raise SetupError("destination parent must be owned by this user and not writable by others")
    return path


def prepare(request):
    """Read-only plan and private inputs; plan digest binds exact input bytes."""
    if not isinstance(request, dict):
        raise SetupError("setup request must be an object")
    kind = request.get("kind")
    optional = ("existing_config",)
    if kind == "provider":
        fields(request, ("kind", "destination", "provider_id", "provider_url", "requester_id",
                         "offer_hashes", "admission_token_file", "allowances"),
               (*optional, "allow_loopback"))
    elif kind == "requester":
        fields(request, ("kind", "destination", "handoff_file", "expected_provider_id",
                         "expected_requester_id"), optional)
    else:
        raise SetupError("setup kind must be provider or requester")
    output = destination(request["destination"])
    source_hashes = {}
    config = {"clients": [], "providers": []}
    if "existing_config" in request:
        config, source = parse_private(request["existing_config"])
        source_hashes["existing_config"] = digest(source)
    validate_config(config)
    if kind == "provider":
        provider_id = hash_value(request["provider_id"])
        requester_id = hash_value(request["requester_id"])
        provider_url = origin(request["provider_url"], request.get("allow_loopback", False))
        approved_offers = offers(request["offer_hashes"])
        allowances = request["allowances"]
        fields(allowances, ("max_total_quotes", "max_total_deals", "max_total_runtime_ms"))
        if any(not isinstance(limit, int) or isinstance(limit, bool)
               or not 1 <= limit <= 2**63 - 1 for limit in allowances.values()):
            raise SetupError("all cumulative free allowances must be explicit positive finite integers")
        if any(item["requester_id"] == requester_id for item in config["clients"]):
            raise SetupError("requester already configured; review existing scope/rotation separately")
        if len(config["clients"]) == 128:
            raise SetupError("A2A client configuration is full")
        if not isinstance(request["admission_token_file"], str) \
                or Path(request["admission_token_file"]).name in ADMIN_FILES:
            raise SetupError("use an issued invitation file, never an operator/runtime token file")
        raw = private_read(request["admission_token_file"])
        source_hashes["admission_token_file"] = digest(raw)
        try:
            access_token = credential(raw.decode().rstrip("\r\n"), invite=True)
        except UnicodeError:
            raise SetupError("invalid admission credential; value withheld") from None
        handoff = {"schema": SCHEMA, "provider_id": provider_id,
                   "requester_id": requester_id, "provider_url": provider_url,
                   "offer_hashes": approved_offers,
                   "allow_loopback": request.get("allow_loopback", False),
                   "admission_token": access_token}
    else:
        handoff, source = parse_private(request["handoff_file"])
        source_hashes["handoff_file"] = digest(source)
        fields(handoff, ("schema", "provider_id", "requester_id", "provider_url",
                         "offer_hashes", "allow_loopback", "admission_token", "a2a_token"))
        if handoff["schema"] != SCHEMA:
            raise SetupError("unsupported handoff schema")
        provider_id, requester_id = hash_value(handoff["provider_id"]), hash_value(handoff["requester_id"])
        if provider_id != hash_value(request["expected_provider_id"]) \
                or requester_id != hash_value(request["expected_requester_id"]):
            raise SetupError("handoff does not match the independently checked provider/requester identities")
        provider_url = origin(handoff["provider_url"], handoff["allow_loopback"])
        approved_offers = offers(handoff["offer_hashes"])
        credential(handoff["a2a_token"])
        credential(handoff["admission_token"], invite=True)
        if handoff["a2a_token"] == handoff["admission_token"]:
            raise SetupError("A2A and admission credentials must be distinct")
        if any(item["provider_url"] == provider_url for item in config["providers"]):
            raise SetupError("provider origin already configured; review replacement/rotation separately")
        if len(config["providers"]) == 128:
            raise SetupError("A2A provider configuration is full")
        allowances = None
    plan = {"schema": SCHEMA, "kind": kind, "destination": str(output),
            "provider_id": provider_id, "provider_url": provider_url,
            "requester_id": requester_id, "offer_hashes": approved_offers,
            "allow_loopback": handoff["allow_loopback"], "allowances": allowances,
            "preserved_clients": len(config["clients"]), "preserved_providers": len(config["providers"]),
            "admission": "existing provider-wide invitation; no new invitation issued",
            "admission_verified": False, "activation": "explicit Node restart required",
            "request_sha256": digest(encoded(request)), "source_sha256": source_hashes}
    plan["plan_sha256"] = digest(encoded(plan))
    return plan, config, handoff


def private_write(path, value):
    descriptor = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
    with os.fdopen(descriptor, "wb") as output:
        output.write(value)
        output.flush()
        os.fsync(output.fileno())


def apply(request, approved_plan_sha256):
    if not isinstance(request, dict):
        raise SetupError("setup request must be an object")
    if os.name != "posix":
        raise SetupError("this source helper requires POSIX private file permissions and advisory locks")
    import fcntl
    path = destination(request.get("destination"))
    lock_path = path.with_name(path.name + ".froglet-lock")
    descriptor = os.open(lock_path, os.O_RDWR | os.O_CREAT | os.O_NOFOLLOW, 0o600)
    with os.fdopen(descriptor, "rb") as lock:
        meta = os.fstat(lock.fileno())
        if not stat.S_ISREG(meta.st_mode) or meta.st_uid != os.geteuid() \
                or stat.S_IMODE(meta.st_mode) != 0o600:
            raise SetupError("setup lock must be an operator-owned private regular file")
        try:
            fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
        except BlockingIOError:
            raise SetupError("another setup owns this destination; retry after it finishes") from None
        plan, config, handoff = prepare(request)
        if not isinstance(approved_plan_sha256, str) or not HASH.fullmatch(approved_plan_sha256) \
                or approved_plan_sha256 != plan["plan_sha256"]:
            raise SetupError("setup_plan_changed: inputs changed or approval does not match; prepare a new plan")
        if plan["kind"] == "provider":
            token = secrets.token_urlsafe(48)
            prior = {item["token"] for item in config["clients"] + config["providers"]}
            while token == handoff["admission_token"] or token in prior:
                token = secrets.token_urlsafe(48)
            config["clients"].append({"requester_id": plan["requester_id"], "token": token,
                                      "offer_hashes": plan["offer_hashes"]})
            handoff["a2a_token"] = token
        else:
            config["providers"].append({"provider_url": plan["provider_url"],
                                        "token": handoff["a2a_token"],
                                        "allow_loopback": handoff["allow_loopback"]})
        validate_config(config)
        temporary = Path(tempfile.mkdtemp(prefix=".froglet-a2a-", dir=path.parent))
        try:
            private_write(temporary / "a2a.json", encoded(config))
            private_write(temporary / "setup-receipt.json", encoded(plan))
            settings = {"FROGLET_A2A_CONFIG_PATH": str(path / "a2a.json")}
            if plan["kind"] == "provider":
                private_write(temporary / "recipient-handoff.json", encoded(handoff))
                settings["FROGLET_PROVIDER_ACCESS_MODE"] = "invite"
                for key, limit in plan["allowances"].items():
                    settings["FROGLET_PROVIDER_" + key.upper()] = str(limit)
            else:
                private_write(temporary / "access.token", handoff["admission_token"].encode())
            script = "# Source into the operator's Node environment before restarting.\n"
            script += "".join("export " + name + "=" + shlex.quote(value) + "\n"
                              for name, value in settings.items())
            private_write(temporary / "activate.sh", script.encode())
            # Recheck source bytes immediately before publication. No existing
            # configuration is replaced; active settings stay unchanged.
            current, _, _ = prepare(request)
            if current != plan:
                raise SetupError("setup_plan_changed: source changed during apply; nothing activated")
            os.rename(temporary, path)
        finally:
            if temporary.exists():
                shutil.rmtree(temporary)
    return {"status": "prepared", "configuration": str(path / "a2a.json"),
            "receipt": str(path / "setup-receipt.json"), "plan_sha256": plan["plan_sha256"],
            "restart_required": True, "admission_verified": False,
            "private_handoff_file" if plan["kind"] == "provider" else "access_token_file":
            str(path / ("recipient-handoff.json" if plan["kind"] == "provider" else "access.token"))}


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--request", required=True, help="absolute private mode-0600 JSON request")
    action = parser.add_mutually_exclusive_group(required=True)
    action.add_argument("--plan", action="store_true", help="read-only redacted review plan")
    action.add_argument("--approved-plan-sha256", help="apply precisely this reviewed plan into a new private directory")
    arguments = parser.parse_args(argv)
    try:
        request, _ = parse_private(arguments.request)
        result = prepare(request)[0] if arguments.plan else apply(request, arguments.approved_plan_sha256)
        print(json.dumps(result, indent=2, sort_keys=True))
        return 0
    except SetupError as error:
        print(json.dumps({"status": "error", "error": str(error)}))
        return 1
    except OSError:
        print(json.dumps({"status": "error", "error": "private setup filesystem operation failed; no credential source disclosed"}))
        return 1


if __name__ == "__main__":
    sys.exit(main())
