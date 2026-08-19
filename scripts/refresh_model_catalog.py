#!/usr/bin/env python3
"""Check the model catalog against what providers actually serve today.

The catalog is a hand-written Rust table. Models are added to it and never
removed, so it accumulates ids that the provider has since delisted — and a
delisted id does not fail loudly: the request 404s, the fallback chain fires,
and the user sees an error about a completely different model. That is how a
"no endpoints support tool use" message ends up describing a model id that
simply no longer exists.

This reports; it does not edit the catalog. Deciding that a model should go is
a judgement about what users are relying on, and a script that silently
rewrites a source file is how you lose a model someone depended on. Run it,
read it, then make the change deliberately.

Usage:
    python scripts/refresh_model_catalog.py              # report
    python scripts/refresh_model_catalog.py --json       # machine-readable
"""

from __future__ import annotations

import argparse
import json
import os
import pathlib
import re
import sys
import urllib.request

ROOT = pathlib.Path(__file__).resolve().parent.parent
CATALOG = ROOT / "crates" / "freeco-kernel-runtime" / "src" / "model_catalog.rs"

# Providers we can check live. Each needs a models endpoint that lists ids and
# a key we can read locally. Anything not listed here is simply not checked —
# reported as "unverified" rather than guessed at.
PROVIDERS = {
    "openrouter": {
        "url": "https://openrouter.ai/api/v1/models",
        "env": "OPENROUTER_API_KEY",
        # Catalog ids carry an `openrouter/` prefix that the API does not use.
        "strip": "openrouter/",
        # Meta-routers are callable but are not enumerated by /models, so a
        # plain membership test reports them as delisted. Verified by calling
        # openrouter/free successfully while it is absent from the listing.
        "not_listed": {"openrouter/free", "openrouter/auto"},
    },
    "nvidia": {
        "url": "https://integrate.api.nvidia.com/v1/models",
        "env": "NVIDIA_API_KEY",
        "strip": None,
        "not_listed": set(),
        # NVIDIA's /models returns no context_length, so the free models here
        # cannot be ranked by capability. Ranking them anyway produced an
        # alphabetical list dressed up as a recommendation, which is worse than
        # saying nothing. Report them, do not rank them.
        "no_ranking": True,
    },
}


def read_secrets() -> dict[str, str]:
    """Keys from the environment, falling back to the local secrets file."""
    out = dict(os.environ)
    for home in (".freeco-ai", ".openfang"):
        p = pathlib.Path.home() / home / "secrets.env"
        if not p.exists():
            continue
        for line in p.read_text(encoding="utf-8", errors="ignore").splitlines():
            m = re.match(r"\s*([A-Z_][A-Z0-9_]*)\s*=\s*(\S+)", line)
            if m and not out.get(m.group(1)):
                out[m.group(1)] = m.group(2)
    return out


def fetch_live(provider: str, cfg: dict, secrets: dict) -> tuple[dict | None, str]:
    key = secrets.get(cfg["env"], "")
    if not key:
        return None, f"no {cfg['env']} — not checked"
    req = urllib.request.Request(cfg["url"], headers={"Authorization": "Bearer " + key})
    try:
        with urllib.request.urlopen(req, timeout=90) as r:
            body = json.loads(r.read().decode("utf-8"))
    except Exception as exc:  # network, auth, rate limit — all "unverified"
        return None, f"lookup failed: {str(exc)[:60]}"
    return {m["id"]: m for m in body.get("data", [])}, "ok"


def parse_catalog() -> list[dict]:
    """Pull id/provider/context/cost out of the Rust table.

    Deliberately a regex over the source rather than a build step: this has to
    run without a Rust toolchain, on a machine that only wants to know whether
    the list is stale.
    """
    src = CATALOG.read_text(encoding="utf-8")
    out = []
    for block in re.finditer(
        r"ModelCatalogEntry\s*\{(.*?)\n        \}", src, re.S
    ):
        b = block.group(1)

        def field(name, pat=r'"([^"]*)"'):
            m = re.search(rf"{name}:\s*{pat}", b)
            return m.group(1) if m else None

        num = lambda n: (
            lambda m: float(m.group(1).replace("_", "")) if m else None
        )(re.search(rf"{n}:\s*([0-9_.]+)", b))

        mid = field("id")
        if not mid:
            continue
        out.append(
            {
                "id": mid,
                "provider": field("provider"),
                "context": num("context_window"),
                "in_cost": num("input_cost_per_m"),
                "out_cost": num("output_cost_per_m"),
                "tools": "supports_tools: true" in b,
            }
        )
    return out


def capability_score(m: dict) -> float:
    """Rough ordering for 'which free model should we default to'.

    Context window and tool support, nothing cleverer. This is not a benchmark
    and must not be presented as one — it ranks what we can actually observe
    from a models endpoint. A model that cannot call tools is unusable for
    agent work whatever its quality, so that dominates.
    """
    if not m.get("tools"):
        return -1.0
    return (m.get("context") or 0) / 1_000_000.0


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--json", action="store_true", help="machine-readable output")
    args = ap.parse_args()

    secrets = read_secrets()
    catalog = parse_catalog()
    live: dict[str, dict | None] = {}
    notes: dict[str, str] = {}
    for name, cfg in PROVIDERS.items():
        live[name], notes[name] = fetch_live(name, cfg, secrets)

    stale, unverified, ok = [], [], []
    for entry in catalog:
        prov = entry["provider"]
        cfg = PROVIDERS.get(prov)
        served = live.get(prov) if cfg else None
        if served is None:
            unverified.append(entry)
            continue
        probe = entry["id"]
        if cfg["strip"] and probe.startswith(cfg["strip"]):
            probe = probe[len(cfg["strip"]) :]
        if entry["id"] in cfg.get("not_listed", set()):
            ok.append(entry)  # callable, just not enumerated
        else:
            (ok if probe in served else stale).append(entry)

    # Best free model per provider, from what is live right now.
    best: dict[str, list] = {}
    for prov, served in live.items():
        if not served:
            continue
        free = []
        for mid, m in served.items():
            pricing = m.get("pricing") or {}
            is_free = prov == "nvidia" or all(
                float(pricing.get(k, 0) or 0) == 0 for k in ("prompt", "completion")
            )
            if not is_free:
                continue
            free.append(
                {
                    "id": mid,
                    "context": m.get("context_length") or 0,
                    "tools": "tools" in (m.get("supported_parameters") or [])
                    or prov == "nvidia",
                }
            )
        if PROVIDERS[prov].get("no_ranking"):
            # No context data from this provider: list, do not rank.
            best[prov] = {"unranked": [f["id"] for f in free[:8]]}
        else:
            free.sort(key=capability_score, reverse=True)
            best[prov] = free[:5]

    if args.json:
        print(json.dumps({"stale": stale, "ok": len(ok), "best_free": best}, indent=2))
        return 0

    print(f"catalog entries: {len(catalog)}")
    for name in PROVIDERS:
        print(f"  {name:<12} {notes[name]}")
    print()

    if stale:
        print(f"NO LONGER SERVED ({len(stale)}) — these 404 at request time:")
        for e in stale:
            print(f"  {e['provider']:<12} {e['id']}")
        print()
    else:
        print("no stale entries among the providers that could be checked\n")

    if unverified:
        provs = sorted({e["provider"] for e in unverified})
        print(f"unverified ({len(unverified)} entries): {', '.join(provs)}")
        print("  add that provider's key to check these\n")

    for prov, models in best.items():
        if not models:
            continue
        if isinstance(models, dict):
            print(f"free on {prov} (this API reports no context size, so these")
            print("  are listed, not ranked — pick from the catalog instead):")
            for mid in models["unranked"]:
                print(f"    {mid}")
            print()
            continue
        print(f"best free on {prov} (tool-capable, widest context first):")
        for m in models:
            flag = "" if m["tools"] else "  [no tool support - unusable for agents]"
            print(f"  {m['context']:>9,}  {m['id']}{flag}")
        print()

    return 1 if stale else 0


if __name__ == "__main__":
    sys.exit(main())
