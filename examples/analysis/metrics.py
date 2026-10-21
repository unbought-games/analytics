"""Pure, versioned cohort calculations. Input adapter owns export validation."""
from collections import Counter, defaultdict
from datetime import datetime, timedelta, timezone
from decimal import Decimal, InvalidOperation

VERSION = "campaign-cohort/1"
WINDOWS = {"D1": (24, 48), "D7": (168, 192), "D30": (720, 744), "D180": (4320, 4344)}
ACTIONS = {"construct", "navigate"}


def utc(value):
    if not isinstance(value, str) or not value.endswith(("Z", "+00:00")):
        raise ValueError("wall time must be an explicit UTC timestamp")
    result = datetime.fromisoformat(value.replace("Z", "+00:00"))
    if result.utcoffset() != timedelta(0):
        raise ValueError("wall time must be UTC")
    return result


def stamp(value):
    return value.astimezone(timezone.utc).isoformat().replace("+00:00", "Z")


def subject(row):
    return (row["issuer"], row["subject"])


def player(row):
    return (row["application"], row["world"], row["player_id"])


def unique(rows, key):
    seen = {}
    for row in rows:
        identity = key(row)
        if identity in seen and seen[identity] != row:
            raise ValueError("conflicting rows for the same scoped identifier")
        seen[identity] = row
    return list(seen.values())


def sample_fraction(value):
    """Accept lossless PostgreSQL numeric strings, including nested JSON."""
    if type(value) not in (str, int, float):
        raise ValueError("invalid sampling fraction")
    try:
        fraction = Decimal(str(value))
    except InvalidOperation as error:
        raise ValueError("invalid sampling fraction") from error
    if not fraction.is_finite() or not 0 <= fraction <= 1:
        raise ValueError("invalid sampling fraction")
    return fraction


def covers(coverage, start, end):
    return (coverage.get("status") == "complete"
            and sample_fraction(coverage.get("sample_fraction")) == 1
            and utc(coverage["start"]) <= start
            and utc(coverage["end"]) >= end)


def rate(cohort, numerator, denominator, exclusions):
    assert cohort == denominator + sum(exclusions.values())
    return {"cohort_size": cohort, "numerator": numerator,
            "eligible_mature_denominator": denominator,
            "rate": numerator / denominator if denominator else None,
            "exclusions": dict(sorted(exclusions.items()))}


def calculate(data, metadata, cutoff):
    """All time ranges are half-open; cutoff is an exclusive evidence boundary."""
    cutoff = utc(cutoff)
    acquisition = unique(data["acquisition"], lambda r: r["event_id"])
    maps = unique(data["identity_map"], lambda r: (subject(r), r["application"]))
    members = unique(data["memberships"], player)
    events = unique(data["activity"], lambda r: (r["application"], r["event_id"]))
    acquisition = [r for r in acquisition if utc(r["occurred_at"]) < cutoff]
    events = [r for r in events if utc(r["occurred_at"]) < cutoff]
    memberships = defaultdict(list)
    for row in members:
        if utc(row["enrolled_at"]) < cutoff:
            memberships[(row["application"], row["local_account_id"])].append(row)
    mappings = defaultdict(list)
    local_owners = {}
    for row in maps:
        mappings[subject(row)].append(row)
        if row["status"] == "verified":
            key = (row["application"], row["local_account_id"])
            if key in local_owners and local_owners[key] != subject(row):
                raise ValueError("local account maps to multiple issuer/subject identities")
            local_owners[key] = subject(row)
    actions = defaultdict(list)
    for event in events:
        if (event["authoritative"] is True and event["actor"] == "human"
                and event["outcome"] == "applied" and event["action"] in ACTIONS):
            actions[player(event)].append(utc(event["occurred_at"]))
    signups = {}
    for row in acquisition:
        if row["kind"] == "signup" and row["authoritative"] is True:
            identity = subject(row)
            if identity in signups and signups[identity] != row:
                raise ValueError("multiple acquisition facts for one account; resolve corrections before export")
            signups[identity] = row
    campaign_ids = {r["campaign"] for r in acquisition}
    reports = []
    global_complete = all(metadata[n]["status"] == "complete" for n in metadata)

    def account_state(signup):
        links = mappings[subject(signup)]
        if any(r["status"] == "deleted" for r in links):
            return "deleted_identity", [], []
        if not links or any(r["status"] != "verified" for r in links):
            return "unmapped_identity", [], []
        owned = [m for r in links for m in memberships[(r["application"], r["local_account_id"])]]
        played = sorted({t for m in owned for t in actions[player(m)]
                         if t >= utc(m["enrolled_at"])})
        return None, owned, played

    def history(owned, start, end):
        if not global_complete:
            return False
        for name in ("acquisition", "identity_map", "memberships", "activity"):
            if not covers(metadata[name]["coverage"], start, end):
                return False
        return all(covers(m["history"], max(start, utc(m["enrolled_at"])), end)
                   for m in owned if utc(m["enrolled_at"]) < end)

    for campaign in sorted(campaign_ids, key=lambda c: (c is not None, c or "")):
        cohort = [r for r in signups.values() if r["campaign"] == campaign]
        observations = [r for r in acquisition if r["campaign"] == campaign]
        visits = unique([r for r in observations if r["kind"] == "visit"],
                        lambda r: (r["property"], r["visit_id"]))
        visitors = {(r["visitor_namespace"], r["visitor_id"]) for r in visits
                    if r["visitor_id"] is not None}
        returning = {subject(r) for r in observations if r["kind"] == "revisit"
                     and r["authoritative"] is True}
        signup_exclusions = Counter()
        signup_n = signup_d = 0
        for visit in visits:
            start = utc(visit["occurred_at"])
            end = start + timedelta(days=30)
            if cutoff < end:
                signup_exclusions["immature"] += 1
            elif not visit["linkage_permitted"]:
                signup_exclusions["linkage_unavailable"] += 1
            elif (metadata["acquisition"]["status"] != "complete"
                  or not covers(metadata["acquisition"]["coverage"], start, end)):
                signup_exclusions["history_unavailable"] += 1
            else:
                signup_d += 1
                signup_n += any(r["property"] == visit["property"]
                                and r["visit_id"] == visit["visit_id"]
                                and start <= utc(r["occurred_at"]) < end for r in cohort)
        activation_exclusions = Counter()
        activation_n = activation_d = 0
        states = []
        observed = verified = enrolled = 0
        for signup in cohort:
            reason, owned, played = account_state(signup)
            enrolled += bool(owned)
            start = utc(signup["occurred_at"])
            end = start + timedelta(days=7)
            anchor = played[0] if played else None
            # A signup cannot prove history for players enrolled before it.
            beginning = min([start] + [utc(m["enrolled_at"]) for m in owned])
            anchor_verified = bool(anchor and history(owned, beginning, anchor))
            observed += bool(anchor)
            verified += anchor_verified
            states.append((reason, owned, anchor, anchor_verified, played))
            if reason:
                activation_exclusions[reason] += 1
            elif cutoff < end:
                activation_exclusions["immature"] += 1
            elif not history(owned, start, end):
                activation_exclusions["history_unavailable"] += 1
            else:
                activation_d += 1
                activation_n += any(start <= t < end for t in played)
        retention = {}
        for name, (lo, hi) in WINDOWS.items():
            exclusions = Counter()
            numerator = denominator = 0
            for reason, owned, anchor, anchor_verified, played in states:
                if reason:
                    exclusions[reason] += 1
                elif anchor is None:
                    exclusions["no_observed_meaningful_play"] += 1
                elif not anchor_verified:
                    exclusions["first_play_history_unavailable"] += 1
                else:
                    start, end = anchor + timedelta(hours=lo), anchor + timedelta(hours=hi)
                    if cutoff < end:
                        exclusions["immature"] += 1
                    elif not history(owned, start, end):
                        exclusions["return_history_unavailable"] += 1
                    else:
                        denominator += 1
                        numerator += any(start <= t < end for t in played)
            retention[name] = {**rate(len(cohort), numerator, denominator, exclusions),
                               "elapsed_hours": [lo, hi]}
        reports.append({"campaign": campaign, "observed_visits": len(visits),
                        "observed_browser_identities": len(visitors),
                        "visits_without_browser_identity": sum(r["visitor_id"] is None for r in visits),
                        "acquired_accounts": len(cohort), "revisiting_accounts": len(returning),
                        "enrolled_accounts": enrolled,
                        "world_memberships": sum(len(s[1]) for s in states),
                        "first_observed_play_accounts": observed,
                        "verified_first_ever_play_accounts": verified,
                        "signup_conversion": rate(len(visits), signup_n, signup_d, signup_exclusions),
                        "activation": rate(len(cohort), activation_n, activation_d, activation_exclusions),
                        "retention": retention})
    return {"definition_version": VERSION, "cutoff": stamp(cutoff),
            "evidence_semantics": "occurrence time < cutoff; source reads may be later (restated history)",
            "campaigns": reports,
            "limitations": ["Observed browser identities are not unique people.",
                            "D180 is a one-day elapsed window, not six calendar months or continuous activity.",
                            "Missing mappings and incomplete history are excluded, never treated as inactivity.",
                            "Campaign association does not establish causation."]}
