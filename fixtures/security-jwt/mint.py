#!/usr/bin/env python3
"""Mints the tokens `tokens.json` holds, with PyJWT and `cryptography`, tools
that share no code with the verifier under test (ADR 0090). Keys are made
fresh each run, so a rerun changes the file; the tests read whatever it
holds. Every valid token expires in 2100, so none depends on the clock.

    python3 fixtures/security-jwt/mint.py > fixtures/security-jwt/tokens.json
"""
import base64
import hashlib
import hmac
import json

import jwt
from cryptography.hazmat.primitives import serialization
from cryptography.hazmat.primitives.asymmetric import ec, rsa

FUTURE = 4102444800  # 2100-01-01
PAST = 1000000000  # 2001


def pem(public):
    return public.public_bytes(
        serialization.Encoding.PEM, serialization.PublicFormat.SubjectPublicKeyInfo
    ).decode()


def b64u(data):
    return base64.urlsafe_b64encode(data).rstrip(b"=").decode()


secret = b"0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"
rsa_key = rsa.generate_private_key(public_exponent=65537, key_size=2048)
p256 = ec.generate_private_key(ec.SECP256R1())
p384 = ec.generate_private_key(ec.SECP384R1())
claims = {"sub": "jwt-user", "exp": FUTURE, "roles": ["analyst"], "iss": "issuer-a", "aud": "aud-a"}


def mint(payload, key, alg):
    return jwt.encode(payload, key, algorithm=alg)


def tamper(token):
    head, body, sig = token.split(".")
    raw = bytearray(base64.urlsafe_b64decode(sig + "=" * (-len(sig) % 4)))
    raw[0] ^= 1
    return ".".join([head, body, b64u(bytes(raw))])


def hs256_by_hand(payload, key):
    # PyJWT refuses a public key as a secret, so the attack is built by hand.
    head = b64u(b'{"alg":"HS256","typ":"JWT"}')
    body = b64u(json.dumps(payload).encode())
    sig = hmac.new(key, f"{head}.{body}".encode(), hashlib.sha256).digest()
    return f"{head}.{body}.{b64u(sig)}"


hs = mint(claims, secret, "HS256")
out = {
    "hs_secret": base64.b64encode(secret).decode(),
    "rsa_public": pem(rsa_key.public_key()),
    "p256_public": pem(p256.public_key()),
    "p384_public": pem(p384.public_key()),
    "valid": {
        "HS256": hs,
        "HS384": mint(claims, secret, "HS384"),
        "HS512": mint(claims, secret, "HS512"),
        "RS256": mint(claims, rsa_key, "RS256"),
        "RS512": mint(claims, rsa_key, "RS512"),
        "ES256": mint(claims, p256, "ES256"),
        "ES384": mint(claims, p384, "ES384"),
    },
    "invalid": {
        "tampered": tamper(hs),
        "truncated": hs.rsplit(".", 1)[0],
        "expired": mint({**claims, "exp": PAST}, secret, "HS256"),
        "not_yet_valid": mint({**claims, "nbf": FUTURE - 1}, secret, "HS256"),
        "wrong_issuer": mint({**claims, "iss": "issuer-b"}, secret, "HS256"),
        "wrong_audience": mint({**claims, "aud": ["aud-b"]}, secret, "HS256"),
        "no_exp": mint({k: v for k, v in claims.items() if k != "exp"}, secret, "HS256"),
        "no_subject": mint({k: v for k, v in claims.items() if k != "sub"}, secret, "HS256"),
        "roles_not_strings": mint({**claims, "roles": [1, 2]}, secret, "HS256"),
        "too_many_roles": mint({**claims, "roles": [f"r{i}" for i in range(65)]}, secret, "HS256"),
        "subject_admin": mint({**claims, "sub": "admin"}, secret, "HS256"),
        "subject_stored_user": mint({**claims, "sub": "u"}, secret, "HS256"),
        "alg_none": b64u(b'{"alg":"none","typ":"JWT"}') + "." + b64u(json.dumps(claims).encode()) + ".",
        # An RSA public key used as an HMAC secret: the classic confusion.
        "rsa_public_as_hmac_secret": hs256_by_hand(claims, pem(rsa_key.public_key()).encode()),
        "wrong_algorithm_for_key": mint(claims, rsa_key, "RS256"),
        "oversize": mint({**claims, "pad": "x" * 9000}, secret, "HS256"),
    },
    "other_roles": mint({**claims, "roles": ["ops"]}, secret, "HS256"),
    "no_roles_claim": mint({k: v for k, v in claims.items() if k != "roles"}, secret, "HS256"),
}
print(json.dumps(out, indent=1))
