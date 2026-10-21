"""Join property-scoped exports without rewriting original acquisition facts."""
from metrics import unique, utc

VERSION = "attributed-visit/1"


def normalize_acquisition(acquisitions, visits, touches):
    """Resolve the global engine touch key, then the property-scoped visit key.

    A touch is not a visit: the selected tagged touch may occur after an untagged
    landing or a landing tagged with a different campaign. No browser identity,
    matching account UUID, or coincidentally equal visit UUID establishes a join.
    """
    acquisitions = unique([dict(r) for r in acquisitions], lambda r: (r["property"], r["event_id"]))
    visits = unique([r for r in acquisitions if r["kind"] == "visit"] + visits,
                    lambda r: (r["property"], r["event_id"]))
    if any(r["kind"] != "visit" for r in visits):
        raise ValueError("acquisition_visits must contain observed visits only")
    visits = unique(visits, lambda r: (r["property"], r["visit_id"]))
    by_visit = {(r["property"], r["visit_id"]): r for r in visits}
    # The ingestion schema owns globally unique touch IDs; these are NOT event IDs.
    by_touch = {r["touch_id"]: r for r in unique(touches, lambda r: r["touch_id"])}
    matched = unresolved = 0
    for signup in acquisitions:
        if signup["kind"] != "signup" or signup["authoritative"] is not True:
            continue
        touch_id = signup.get("touch_id")
        if touch_id is None:
            continue
        touch = by_touch.get(touch_id)
        origin = by_visit.get((touch["property"], touch["visit_id"])) if touch else None
        if (touch is None or origin is None or not touch["linkage_permitted"]
                or not origin["linkage_permitted"]):
            unresolved += 1
            continue
        if (touch["campaign"] != signup["campaign"]
                or not utc(origin["occurred_at"]) <= utc(touch["occurred_at"]) <= utc(signup["occurred_at"])):
            raise ValueError("selected touch disagrees with acquisition campaign or journey clock")
        if signup.get("attributed_visit_verified") is True and (signup["attributed_visit_property"], signup["attributed_visit_id"]) != (origin["property"], origin["visit_id"]):
            raise ValueError("exported attributed visit contradicts selected touch")
        signup.update(attributed_visit_property=origin["property"],
                      attributed_visit_id=origin["visit_id"], attributed_visit_verified=True)
        matched += 1
    return ([r for r in acquisitions if r["kind"] != "visit"] + visits,
            {"version": VERSION, "matched_signups": matched, "unresolved_selected_touches": unresolved,
             "source_semantics": "selected engine touch ID to original property-scoped visit; signup facts preserved"})
