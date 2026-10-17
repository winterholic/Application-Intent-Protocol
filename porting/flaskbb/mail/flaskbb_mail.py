"""FlaskBB account-email worker (F02/F14).

Declared in app.aip as `operation job sendActivationEmail` and
`sendPasswordResetEmail` with `implementation "flaskbb_mail.send_*"` and
`access http.mail`. The worker runs with its network denied, so it never opens
a socket (the original's Flask-Mail SMTP path cannot work here). Delivery goes
through the server-side outbound HTTP broker: `ctx.http.call("mail", payload,
key)` reaches the one endpoint named `mail` in the service config
(`workers.integrations`), whose URL, method, size limits and credentials are
deployment facts the job cannot choose.

The `mail` endpoint is a provider HTTP API. This module builds the Mailpit
`POST /api/v1/send` JSON shape (the isolated test fixture); a production
deployment points `mail` at its provider and adapts `build_payload` to that
provider's request schema, which is the only provider-specific code.

The sender address and the link base URL are non-secret deployment settings
passed through `workers.environment` (FLASKBB_MAIL_SENDER,
FLASKBB_ACTIVATION_BASE_URL); nothing is hardcoded and the job never reads rows
-- the caller passes the already-authorized values.

Message building is pure (build_* functions) so content stays testable without
a socket or a broker.
"""

import hashlib
import os

_OK = range(200, 300)


def _config():
    sender = os.environ.get("FLASKBB_MAIL_SENDER", "")
    base = os.environ.get("FLASKBB_ACTIVATION_BASE_URL", "http://127.0.0.1:58080")
    if not sender:
        raise RuntimeError("mail not configured, missing: FLASKBB_MAIL_SENDER")
    return sender, base


def build_payload(email_addr, username, subject, text):
    """Provider request body (Mailpit send API shape)."""
    sender, _ = _config()
    return {
        "From": {"Email": sender, "Name": "FlaskBB"},
        "To": [{"Email": email_addr, "Name": username}],
        "Subject": subject,
        "Text": text,
    }


def build_activation_email(email_addr, username, token):
    """Pure message construction for the activation mail."""
    _, base = _config()
    link = f"{base}/auth/activate/{token}"
    return build_payload(
        email_addr,
        username,
        "Activate your FlaskBB account",
        f"Hello {username},\n\n"
        f"activate your FlaskBB account here:\n{link}\n\n"
        "If you did not register, ignore this message.\n",
    )


def build_password_reset_email(email_addr, username, token):
    """Pure message construction for the password-reset mail."""
    _, base = _config()
    link = f"{base}/auth/reset/{token}"
    return build_payload(
        email_addr,
        username,
        "Reset your FlaskBB password",
        f"Hello {username},\n\n"
        f"reset your FlaskBB password here:\n{link}\n\n"
        "If you did not ask for a reset, ignore this message.\n",
    )


def effect_key(kind, email_addr, token):
    """Stable idempotency key: a retry of the same mail replays instead of resending."""
    digest = hashlib.sha256(f"{kind}\0{email_addr}\0{token}".encode()).hexdigest()
    return f"mail-{kind}-{digest[:40]}"


async def _deliver(ctx, kind, payload, email_addr, token):
    reply = await ctx.http.call("mail", payload, effect_key(kind, email_addr, token))
    if reply["status"] not in _OK:
        raise RuntimeError(f"mail provider rejected the message with status {reply['status']}")
    return {"accepted": True}


async def send_activation_email(input, ctx):
    """Deliver the account-activation link. Returns {"accepted": True}."""
    payload = build_activation_email(input["email"], input["username"], input["token"])
    return await _deliver(ctx, "activation", payload, input["email"], input["token"])


async def send_password_reset_email(input, ctx):
    """Deliver the password-reset link. Returns {"accepted": True}."""
    payload = build_password_reset_email(input["email"], input["username"], input["token"])
    return await _deliver(ctx, "reset", payload, input["email"], input["token"])
