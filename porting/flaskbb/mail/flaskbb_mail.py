"""FlaskBB account-email worker (F02/F14).

Declared in app.aip as `operation job sendActivationEmail` and
`sendPasswordResetEmail` with `implementation "flaskbb_mail.send_*"`.
Shape follows the repo worker convention: `async def fn(input, ctx)`
returning the declared output. SMTP targets come only from the deployment
environment (FLASKBB_SMTP_*); nothing is hardcoded and the job never reads
rows itself -- the caller passes the already-authorized values.

Message building is pure (build_* functions) so content stays testable
without a socket; only deliver() touches the network.
"""

import asyncio
import email.message
import os
import smtplib


def _config():
    host = os.environ.get("FLASKBB_SMTP_HOST", "")
    port = int(os.environ.get("FLASKBB_SMTP_PORT", "0") or 0)
    sender = os.environ.get("FLASKBB_SMTP_SENDER", "")
    base = os.environ.get("FLASKBB_ACTIVATION_BASE_URL", "http://127.0.0.1:58080")
    missing = [k for k, v in (("FLASKBB_SMTP_HOST", host), ("FLASKBB_SMTP_PORT", port), ("FLASKBB_SMTP_SENDER", sender)) if not v]
    if missing:
        raise RuntimeError(f"mail not configured, missing: {', '.join(missing)}")
    return host, port, sender, base


def build_activation_email(email_addr, username, token):
    """Pure message construction for the activation mail."""
    _, _, sender, base = _config()
    link = f"{base}/auth/activate/{token}"
    msg = email.message.EmailMessage()
    msg["From"] = sender
    msg["To"] = email_addr
    msg["Subject"] = "Activate your FlaskBB account"
    msg.set_content(
        f"Hello {username},\n\n"
        f"activate your FlaskBB account here:\n{link}\n\n"
        "If you did not register, ignore this message.\n"
    )
    return msg


def build_password_reset_email(email_addr, username, token):
    """Pure message construction for the password-reset mail."""
    _, _, sender, base = _config()
    link = f"{base}/auth/reset/{token}"
    msg = email.message.EmailMessage()
    msg["From"] = sender
    msg["To"] = email_addr
    msg["Subject"] = "Reset your FlaskBB password"
    msg.set_content(
        f"Hello {username},\n\n"
        f"reset your FlaskBB password here:\n{link}\n\n"
        "If you did not ask for a reset, ignore this message.\n"
    )
    return msg


def deliver(msg):
    """Blocking SMTP delivery of a built message (runs in a worker thread)."""
    host, port, _, _ = _config()

    def _deliver():
        with smtplib.SMTP(host, port, timeout=10) as smtp:
            smtp.send_message(msg)

    return _deliver


async def send_activation_email(input, ctx):
    """Deliver the account-activation link. Returns {"accepted": True}."""
    msg = build_activation_email(input["email"], input["username"], input["token"])
    await asyncio.to_thread(deliver(msg))
    return {"accepted": True}


async def send_password_reset_email(input, ctx):
    """Deliver the password-reset link. Returns {"accepted": True}."""
    msg = build_password_reset_email(input["email"], input["username"], input["token"])
    await asyncio.to_thread(deliver(msg))
    return {"accepted": True}
