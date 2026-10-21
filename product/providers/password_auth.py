"""Password hashing that reads and writes the Werkzeug 3.0.2 layout (`method$salt$hexdigest`).

The original FaceReview stores `werkzeug.security.generate_password_hash` output, and its requirements pin
Werkzeug==3.0.2 (bcrypt is listed but never imported by the service). Only the standard library is used so the same
file can run inside the network-less extension sandbox. Unlike Werkzeug it refuses parameters outside fixed bounds,
because a stored hash is data: an oversized scrypt `n` or pbkdf2 iteration count must not become a memory or CPU bomb.
"""

import hashlib
import hmac
import secrets

SALT_CHARS = "abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789"
DEFAULT_METHOD = "scrypt:32768:8:1"
DEFAULT_PBKDF2_ITERATIONS = 600000

_SCRYPT_N = (1024, 2**17)
_SCRYPT_R = (1, 16)
_SCRYPT_P = (1, 4)
_PBKDF2_ITERATIONS = (1, 5_000_000)
_PBKDF2_HASHES = ("sha1", "sha256", "sha512")
_MAX_PASSWORD_BYTES = 4096
_MAX_SALT_BYTES = 128


def _digest(method, salt, password):
    """Returns (hex digest, canonical method) or raises ValueError for anything outside the supported bounds."""
    name, *args = method.split(":")
    salt_bytes, secret = salt.encode(), password.encode()
    if not 0 < len(salt_bytes) <= _MAX_SALT_BYTES or len(secret) > _MAX_PASSWORD_BYTES:
        raise ValueError("size")
    if name == "scrypt":
        if args and len(args) != 3:
            raise ValueError("arity")
        n, r, p = map(int, args) if args else (2**15, 8, 1)
        if not (_SCRYPT_N[0] <= n <= _SCRYPT_N[1] and n & (n - 1) == 0 and _SCRYPT_R[0] <= r <= _SCRYPT_R[1] and _SCRYPT_P[0] <= p <= _SCRYPT_P[1]):
            raise ValueError("scrypt bounds")
        # Same headroom Werkzeug asks for; hashlib needs a little more than 128 * n * r.
        digest = hashlib.scrypt(secret, salt=salt_bytes, n=n, r=r, p=p, maxmem=132 * n * r * p)
        return digest.hex(), f"scrypt:{n}:{r}:{p}"
    if name == "pbkdf2":
        if len(args) > 2:
            raise ValueError("arity")
        hash_name = args[0] if args else "sha256"
        iterations = int(args[1]) if len(args) == 2 else DEFAULT_PBKDF2_ITERATIONS
        if hash_name not in _PBKDF2_HASHES or not _PBKDF2_ITERATIONS[0] <= iterations <= _PBKDF2_ITERATIONS[1]:
            raise ValueError("pbkdf2 bounds")
        return hashlib.pbkdf2_hmac(hash_name, secret, salt_bytes, iterations).hex(), f"pbkdf2:{hash_name}:{iterations}"
    raise ValueError("method")


def generate_password_hash(password, method=DEFAULT_METHOD, salt_length=16):
    salt = "".join(secrets.choice(SALT_CHARS) for _ in range(salt_length))
    digest, canonical = _digest(method, salt, password)
    return f"{canonical}${salt}${digest}"


def check_password_hash(stored, password):
    if not isinstance(stored, str) or not isinstance(password, str):
        return False
    try:
        method, salt, expected = stored.split("$", 2)
        actual, _ = _digest(method, salt, password)
    except (ValueError, UnicodeError, OverflowError, MemoryError):
        return False
    return hmac.compare_digest(actual, expected)


def needs_rehash(stored):
    """True when `stored` is not the current default method, so a successful login should rewrite it."""
    try:
        method = stored.split("$", 1)[0]
        _, canonical = _digest(method, "x", "x")
    except (ValueError, AttributeError, MemoryError):
        return True
    return canonical != DEFAULT_METHOD


async def hash_password(input, ctx):
    """Extension entry for signup: the server hashes a new password here and stores only the result. The server bounds the
    password length before calling; this refuses a non-text value the same way."""
    password = input.get("password")
    if not isinstance(password, str):
        raise ValueError("password must be text")
    return {"hash": generate_password_hash(password)}


async def verify_password(input, ctx):
    """Extension entry. The server supplies the stored hash from its own read of the credential column; the extension
    answers only with a verdict so neither the hash nor the password leaves the sandbox."""
    ok = check_password_hash(input.get("hash"), input.get("password"))
    return {"ok": ok, "rehash": ok and needs_rehash(input["hash"])}
