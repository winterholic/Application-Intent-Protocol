"""Refuses PostgreSQL defaults for the prototype smoke tools.

These tools create and drop schemas, so they must only touch a dedicated test database. The rules match
`product/test-support/postgres-guard.mjs` and `crates/aip-test-support`: the host, an explicit high port other than 5432, the user and a
database named `aip_test_*` must all be spelled out, and no `PG*` environment variable may redirect the connection. With no `--db-url`
the DSN comes from `AIP_TEST_DATABASE_URL`; there is no built-in default.
"""
import os
import re
import shlex
from urllib.parse import parse_qsl, unquote, urlparse

_IDENTITY = {"host", "hostaddr", "port", "user", "dbname", "service", "servicefile"}


class DsnRefused(SystemExit):
    def __init__(self, reason):
        super().__init__(f"refusing PostgreSQL target: {reason}. Pass --db-url or set AIP_TEST_DATABASE_URL to a dedicated aip_test_* database on an explicit high port.")


def _parse(raw):
    if re.match(r"^postgres(?:ql)?://", raw, re.I):
        url = urlparse(raw)
        for key, _ in parse_qsl(url.query, keep_blank_values=True):
            if key.lower() in _IDENTITY:
                raise DsnRefused("connection identity parameters must appear only once in the URI authority and path")
        fields = {}
        if url.hostname:
            fields["host"] = url.hostname
        if url.port:
            fields["port"] = str(url.port)
        if url.username:
            fields["user"] = unquote(url.username)
        if url.path.lstrip("/"):
            fields["dbname"] = unquote(url.path.lstrip("/"))
        return fields
    fields = {}
    try:
        tokens = shlex.split(raw)
    except ValueError:
        raise DsnRefused("the keyword DSN is malformed") from None
    for token in tokens:
        key, separator, value = token.partition("=")
        if not separator or not key:
            raise DsnRefused("the keyword DSN is malformed")
        if key.lower() in fields:
            raise DsnRefused("duplicate DSN keys are not allowed")
        fields[key.lower()] = value
    return fields


def validate(raw):
    """Returns `raw` when it names a dedicated test database, otherwise exits with the reason."""
    if not isinstance(raw, str) or not raw.strip():
        raise DsnRefused("no DSN given")
    fields = _parse(raw)
    if "service" in fields or "servicefile" in fields:
        raise DsnRefused("libpq service-file fallback is not allowed")
    host = fields.get("host")
    if not host:
        raise DsnRefused("the host must be explicit")
    if "," in host or "hostaddr" in fields:
        raise DsnRefused("multiple or indirect hosts are not allowed")
    if "port" not in fields:
        raise DsnRefused("the port must be explicit")
    if not fields["port"].isdigit() or int(fields["port"]) <= 1024 or int(fields["port"]) == 5432:
        raise DsnRefused("the port must be a dedicated non-default high port")
    if not fields.get("user"):
        raise DsnRefused("the user must be explicit")
    if not fields.get("dbname", "").startswith("aip_test_"):
        raise DsnRefused("the database name must start with aip_test_")
    return raw


def resolve(argument):
    """The DSN for a smoke run: the explicit argument or AIP_TEST_DATABASE_URL, validated. PG* variables must be unset."""
    for name in os.environ:
        if name.startswith("PG"):
            raise DsnRefused(f"{name} must be unset so it cannot redirect the connection")
    return validate(argument or os.environ.get("AIP_TEST_DATABASE_URL", ""))
