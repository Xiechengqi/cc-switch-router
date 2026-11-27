#!/usr/bin/env python3
"""Prepare a public recovery request using read-only local evidence.

This does not invoke the Router API, change a database, or edit Server config.
The authenticated recovery preview remains the authority for live obligations.
"""

import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import sqlite3
import sys
from urllib.parse import quote
from uuid import UUID


def require(condition, message):
    if not condition:
        raise ValueError(message)


def read_database(path, *, immutable=False):
    uri = "file:" + quote(str(Path(path).resolve()), safe="/") + "?mode=ro"
    if immutable:
        uri += "&immutable=1"
    connection = sqlite3.connect(uri, uri=True)
    connection.row_factory = sqlite3.Row
    connection.execute("PRAGMA query_only=ON")
    connection.execute("BEGIN")
    return connection


def verified_installation(connection, installation_id, public_key, email):
    row = connection.execute(
        "SELECT public_key,owner_email,owner_verified_at,lifecycle FROM installations WHERE id=?",
        (installation_id,),
    ).fetchone()
    require(row is not None, "evidence installation is missing")
    require(row["public_key"] == public_key, "evidence public keys do not match")
    require(row["owner_email"] == email and row["owner_verified_at"], "verified owners do not match")
    require(row["lifecycle"] == "active", "evidence installation is fenced")


def prepare(args):
    require(str(UUID(args.original_id)) == args.original_id, "original ID must be a canonical UUID")
    require(str(UUID(args.replacement_id)) == args.replacement_id, "replacement ID must be a canonical UUID")
    require(args.original_id != args.replacement_id, "original and replacement IDs must differ")
    require(re.fullmatch(r"[a-z0-9](?:[a-z0-9-]{0,61}[a-z0-9])?", args.subdomain), "invalid Client subdomain")
    config = json.loads(Path(args.server_config).read_text(encoding="utf-8"))
    identity = config.get("router", {}).get("identity") or {}
    public_key = identity.get("publicKey")
    email = config.get("owner", {}).get("email")
    require(public_key and email, "Server public key or owner is missing")
    require(identity.get("installationId") == args.replacement_id, "Server ID differs from replacement ID")
    require(config.get("client", {}).get("tunnelSubdomain") == args.subdomain, "Server Client subdomain differs")

    evidence = []
    for path, expected_hash in args.trusted_backup:
        require(re.fullmatch(r"[0-9a-f]{64}", expected_hash), "backup SHA256 must be 64 lowercase hex characters")
        digest = hashlib.sha256()
        with open(path, "rb") as source:
            for chunk in iter(lambda: source.read(1024 * 1024), b""):
                digest.update(chunk)
        require(digest.hexdigest() == expected_hash, "trusted backup SHA256 changed")
        # Only the standalone backup file is covered by its supplied SHA256.
        # A separate WAL beside it cannot serve as identity evidence.
        backup = read_database(path, immutable=True)
        try:
            verified_installation(backup, args.original_id, public_key, email)
            tunnel = backup.execute(
                "SELECT subdomain FROM installation_client_tunnels WHERE installation_id=?",
                (args.original_id,),
            ).fetchone()
            require(tunnel and tunnel["subdomain"] == args.subdomain, "backup Client subdomain does not match")
        finally:
            backup.close()
        evidence.append("backup=" + str(Path(path).resolve()) + ";sha256=" + expected_hash)

    live = read_database(args.router_db)
    try:
        verified_installation(live, args.replacement_id, public_key, email)
        require(not live.execute("SELECT 1 FROM installations WHERE id=?", (args.original_id,)).fetchone(), "original runtime already exists")
        anchor = live.execute("SELECT installation_id,lifecycle,owner_email,owner_verified_at FROM client_identities WHERE public_key=?", (public_key,)).fetchone()
        require(anchor and anchor["installation_id"] == args.replacement_id and anchor["lifecycle"] == "active"
                and anchor["owner_email"] == email and anchor["owner_verified_at"], "durable replacement identity changed")
        host = live.execute(
            "SELECT kind,subject_id,installation_id,route_id,target_lane_id,lifecycle,revision "
            "FROM public_hosts WHERE label=?", (args.subdomain,),
        ).fetchone()
        require(host and host["kind"] == "client" and host["subject_id"] == args.original_id
                and host["route_id"] == "client:" + args.original_id and host["target_lane_id"] == args.original_id
                and host["installation_id"] == args.original_id and host["lifecycle"] != "tombstoned"
                and host["revision"] == args.expected_host_revision, "reserved Client host changed")
    finally:
        live.close()
    evidence = " | ".join(evidence)
    require(len(evidence.encode("utf-8")) <= 1024, "backup evidence is too long for the recovery API")
    return {
        "originalInstallationId": args.original_id,
        "replacementInstallationId": args.replacement_id,
        "publicKey": public_key,
        "subdomain": args.subdomain,
        "expectedHostRevision": args.expected_host_revision,
        "expectedOwnerEmail": email,
        "evidence": evidence,
        "apply": False,
    }


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--router-db", required=True)
    parser.add_argument("--server-config", required=True)
    parser.add_argument("--trusted-backup", nargs=2, metavar=("PATH", "SHA256"), action="append", required=True)
    parser.add_argument("--original-id", required=True)
    parser.add_argument("--replacement-id", required=True)
    parser.add_argument("--subdomain", required=True)
    parser.add_argument("--expected-host-revision", type=int, required=True)
    parser.add_argument("--output", help="new request file (0600); defaults to stdout")
    args = parser.parse_args()
    try:
        request = json.dumps(prepare(args), ensure_ascii=False, indent=2) + "\n"
        if args.output:
            descriptor = os.open(args.output, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
            with os.fdopen(descriptor, "w", encoding="utf-8") as target:
                target.write(request)
        else:
            sys.stdout.write(request)
    except (ValueError, OSError, sqlite3.Error, TypeError) as error:
        print("recovery request preparation failed: " + str(error), file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
