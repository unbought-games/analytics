"""A09/A10 behavior tests against manufactured files and exact clocks."""
import copy
from datetime import timedelta
import importlib.util
import json
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

from manifest_adapter import digest, load_manifest, parse_json, utc
from metrics import calculate, WINDOWS
from normalize import normalize_acquisition

ROOT = Path(__file__).resolve().parents[2]
FIXTURE_ROOT = ROOT / "tests/fixtures/cohorts"
spec = importlib.util.spec_from_file_location("cohort_generator", FIXTURE_ROOT / "generate.py")
generator = importlib.util.module_from_spec(spec)
spec.loader.exec_module(generator)
CUTOFF = generator.timestamp(generator.CUTOFF)


def campaign(report, name=generator.CAMPAIGN):
    return next(c for c in report["campaigns"] if c["campaign"] == name)


class CohortTests(unittest.TestCase):
    def setUp(self):
        self.data, self.meta, _ = load_manifest(FIXTURE_ROOT / "manufactured/manifest.json", CUTOFF)

    def report(self, cutoff=CUTOFF):
        return campaign(calculate(self.data, self.meta, cutoff))

    def solo(self):
        account = next(r for r in self.data["acquisition"] if r["event_id"] == generator.uid("signup/alpha"))
        member = next(r for r in self.data["memberships"] if r["player_id"] == generator.uid("player/alpha-delta"))
        mapping = next(r for r in self.data["identity_map"] if r["local_account_id"] == member["local_account_id"])
        action = next(r for r in self.data["activity"] if r["event_id"] == generator.uid("action/alpha-first"))
        self.data = {"acquisition": [account], "identity_map": [mapping], "memberships": [member], "activity": [action]}
        return action

    def test_manufactured_expected_metrics(self):
        report = self.report()
        self.assertEqual([report[k] for k in ("acquired_accounts", "observed_visits", "revisiting_accounts",
                                               "enrolled_accounts", "world_memberships")], [10, 12, 1, 7, 9])
        self.assertEqual(report["activation"]["rate"], 4 / 6)
        self.assertEqual(report["signup_conversion"]["rate"], 9 / 10)
        for name in ("D1", "D7", "D30"):
            self.assertEqual(report["retention"][name]["rate"], 1 / 4)
        self.assertEqual(report["retention"]["D180"]["rate"], 1 / 3)
        self.assertEqual(report["first_observed_play_accounts"], 6)
        self.assertEqual(report["verified_first_ever_play_accounts"], 5)

    def test_all_exact_action_boundaries(self):
        first = self.solo()
        for name, (lower, upper) in WINDOWS.items():
            for offset, expected in ((timedelta(hours=lower) - timedelta(microseconds=1), 0),
                                     (timedelta(hours=lower), 1),
                                     (timedelta(hours=upper) - timedelta(microseconds=1), 1),
                                     (timedelta(hours=upper), 0)):
                with self.subTest(window=name, offset=offset):
                    event = dict(first, event_id="boundary", occurred_at=generator.timestamp(generator.ANCHOR + offset))
                    self.data["activity"] = [first, event]
                    self.assertEqual(self.report()["retention"][name]["numerator"], expected)

    def test_all_exact_maturity_boundaries(self):
        self.solo()
        for name, (_, end) in WINDOWS.items():
            for delta, mature in ((timedelta(microseconds=-1), False), (timedelta(0), True)):
                with self.subTest(window=name, delta=delta):
                    cutoff = generator.timestamp(generator.ANCHOR + timedelta(hours=end) + delta)
                    metric = self.report(cutoff)["retention"][name]
                    self.assertEqual(metric["eligible_mature_denominator"], int(mature))
                    self.assertEqual(metric["rate"], 0 if mature else None)

    def test_activation_start_end_and_maturity(self):
        first = self.solo()
        for offset, expected in ((timedelta(0), 1), (timedelta(days=7) - timedelta(microseconds=1), 1),
                                 (timedelta(days=7), 0)):
            with self.subTest(offset=offset):
                self.data["activity"] = [dict(first, occurred_at=generator.timestamp(generator.START + offset))]
                self.assertEqual(self.report()["activation"]["numerator"], expected)
        self.assertIsNone(self.report(generator.timestamp(generator.START + timedelta(days=7, microseconds=-1)))["activation"]["rate"])
        self.assertEqual(self.report(generator.timestamp(generator.START + timedelta(days=7)))["activation"]["eligible_mature_denominator"], 1)

    def test_signup_window_and_linkage(self):
        self.solo()
        signup = self.data["acquisition"][0]
        visit = {"event_id": "visit", "kind": "visit", "campaign": generator.CAMPAIGN,
                 "occurred_at": generator.timestamp(generator.START), "property": signup["property"],
                 "visit_id": signup["visit_id"], "visitor_namespace": "portal/browser", "visitor_id": "browser",
                 "linkage_permitted": True}
        self.data["acquisition"].append(visit)
        for offset, expected in ((timedelta(0), 1), (timedelta(days=30, microseconds=-1), 1), (timedelta(days=30), 0)):
            signup["occurred_at"] = generator.timestamp(generator.START + offset)
            self.assertEqual(self.report()["signup_conversion"]["numerator"], expected)
        visit["linkage_permitted"] = False
        self.assertIsNone(self.report()["signup_conversion"]["rate"])

    def test_event_ids_are_property_scoped(self):
        self.solo()
        signup = self.data["acquisition"][0]
        self.data["acquisition"].append(dict(signup, property="second-property", subject="second-account"))
        self.assertEqual(self.report()["acquired_accounts"], 2)
        visit = {"event_id": signup["event_id"], "kind": "visit", "campaign": generator.CAMPAIGN,
                 "occurred_at": generator.timestamp(generator.START), "property": "origin-property",
                 "visit_id": "shared-visit", "visitor_namespace": "origin/browser", "visitor_id": "browser",
                 "linkage_permitted": True}
        self.data["acquisition"] += [visit, dict(visit, property="another-origin")]
        self.assertEqual(self.report()["observed_visits"], 2)

    def test_explicit_attributed_visit_keeps_signup_scope(self):
        self.solo()
        signup = self.data["acquisition"][0]
        visit = {"event_id": "landing", "kind": "visit", "campaign": "landing-campaign",
                 "occurred_at": generator.timestamp(generator.START), "property": "origin",
                 "visit_id": "origin-visit", "visitor_namespace": "origin/browser", "visitor_id": "browser",
                 "linkage_permitted": True}
        self.data["acquisition"].append(visit)
        signup.update(property="identity", visit_id="auth-visit", attributed_visit_property="origin",
                      attributed_visit_id="origin-visit", attributed_visit_verified=True)
        result = calculate(self.data, self.meta, CUTOFF)
        self.assertEqual(campaign(result, "landing-campaign")["signup_conversion"]["numerator"], 1)
        self.assertEqual(campaign(result)["acquired_accounts"], 1)
        self.assertEqual(signup["property"], "identity")
        self.assertEqual(signup["visit_id"], "auth-visit")
        signup["attributed_visit_verified"] = False
        self.assertEqual(campaign(calculate(self.data, self.meta, CUTOFF), "landing-campaign")["signup_conversion"]["numerator"], 0)

    def test_observed_conversion_is_visible_before_maturity_without_invented_coverage(self):
        self.solo()
        signup = self.data["acquisition"][0]
        visit = {"event_id": "landing", "kind": "visit", "campaign": generator.CAMPAIGN,
                 "occurred_at": generator.timestamp(generator.START), "property": signup["property"],
                 "visit_id": signup["visit_id"], "visitor_namespace": "portal/browser", "visitor_id": "browser",
                 "linkage_permitted": True}
        self.data["acquisition"].append(visit)
        self.meta["acquisition"]["coverage"]["status"] = "partial"
        report = self.report(generator.timestamp(generator.START + timedelta(hours=2)))
        self.assertEqual(report["observed_converted_visits"], 1)
        self.assertIsNone(report["signup_conversion"]["rate"])
        self.assertEqual(report["signup_conversion"]["exclusions"], {"immature": 1})

    def test_duplicate_rows_do_not_change_metrics(self):
        expected = self.report()
        for name in self.data:
            self.data[name] += copy.deepcopy(self.data[name])
            self.data[name].reverse()
        self.assertEqual(self.report(), expected)

    def test_conflicting_scoped_ids_fail(self):
        for name in self.data:
            with self.subTest(dataset=name):
                item = dict(self.data[name][0], extra_conflicting_fact=True)
                self.data[name].append(item)
                with self.assertRaises(ValueError):
                    self.report()
                self.data[name].pop()

    def test_ambiguous_local_account_rejected(self):
        self.solo()
        self.data["identity_map"].append(dict(self.data["identity_map"][0], subject="different"))
        with self.assertRaises(ValueError):
            self.report()

    def test_subject_uuid_is_not_local_uuid(self):
        self.solo()
        self.data["memberships"][0]["local_account_id"] = self.data["acquisition"][0]["subject"]
        self.assertEqual(self.report()["first_observed_play_accounts"], 0)

    def test_other_issuer_cannot_inherit_mapping(self):
        self.solo()
        self.data["acquisition"][0]["issuer"] = "https://unmapped-issuer.example"
        self.assertEqual(self.report()["retention"]["D1"]["exclusions"], {"unmapped_identity": 1})

    def test_other_world_or_application_cannot_inherit_action(self):
        first = self.solo()
        for changes in ({"world": "another-world"}, {"application": "another-app"}):
            self.data["activity"] = [dict(first, **changes)]
            self.assertEqual(self.report()["first_observed_play_accounts"], 0)

    def test_postgres_lossless_nested_numeric_strings(self):
        expected = self.report()
        for member in self.data["memberships"]:
            member["history"]["sample_fraction"] = "1"
        self.assertEqual(self.report(), expected)
        self.solo()
        self.data["memberships"][0]["history"]["sample_fraction"] = "0.999999999999999999999999999999999999"
        self.assertIsNone(self.report()["activation"]["rate"])

    def test_deleted_mapping_never_joins(self):
        self.solo()
        self.data["identity_map"][0]["status"] = "deleted"
        result = self.report()
        self.assertEqual(result["first_observed_play_accounts"], 0)
        self.assertEqual(result["activation"]["exclusions"], {"deleted_identity": 1})

    def test_nonhuman_or_unsuccessful_or_observational_never_anchors(self):
        first = self.solo()
        cases = [{"actor": "scheduler"}, {"actor": "background"}, {"actor": "other_player"},
                 {"outcome": "rejected"}, {"action": "inspect"}, {"action": "login"}, {"authoritative": False}]
        for change in cases:
            with self.subTest(change=change):
                self.data["activity"] = [dict(first, **change)]
                self.assertEqual(self.report()["first_observed_play_accounts"], 0)
                self.assertEqual(self.report()["activation"]["rate"], 0)

    def test_global_partial_sampled_and_unavailable_history(self):
        self.solo()
        original = copy.deepcopy(self.meta)
        for name in self.meta:
            for change in ({"status": "unavailable"}, {"sample_fraction": 0.5}, {"start": "2026-01-02T00:00:00Z"}):
                with self.subTest(name=name, change=change):
                    self.meta = copy.deepcopy(original)
                    self.meta[name]["coverage"].update(change)
                    self.assertIsNone(self.report()["activation"]["rate"])
                    self.assertIsNone(self.report()["retention"]["D180"]["rate"])

    def test_expired_anchor_and_late_history_have_distinct_exclusions(self):
        metric = self.report()["retention"]["D180"]
        self.assertEqual(metric["exclusions"]["first_play_history_unavailable"], 1)
        self.assertEqual(metric["exclusions"]["return_history_unavailable"], 1)

    def test_earlier_membership_blocks_false_first_ever(self):
        self.solo()
        self.data["memberships"][0]["enrolled_at"] = "2025-11-01T00:00:00Z"
        self.assertEqual(self.report()["verified_first_ever_play_accounts"], 0)

    def test_revisit_does_not_reassign_acquisition(self):
        report = calculate(self.data, self.meta, CUTOFF)
        self.assertEqual(campaign(report, "earlier-campaign")["acquired_accounts"], 1)
        self.assertEqual(campaign(report)["acquired_accounts"], 10)
        self.assertEqual(campaign(report, None)["acquired_accounts"], 1)

    def test_empty_and_zero_denominator(self):
        self.data = {n: [] for n in self.data}
        self.assertEqual(calculate(self.data, self.meta, CUTOFF)["campaigns"], [])
        self.setUp()
        first = self.solo()
        self.data["activity"] = []
        self.assertIsNone(self.report()["retention"]["D180"]["rate"])

    def test_exclusions_partition_every_cohort(self):
        for report in calculate(self.data, self.meta, CUTOFF)["campaigns"]:
            for metric in (report["activation"], report["signup_conversion"], *report["retention"].values()):
                self.assertEqual(metric["cohort_size"], metric["eligible_mature_denominator"] + sum(metric["exclusions"].values()))
                self.assertLessEqual(metric["numerator"], metric["eligible_mature_denominator"])

    def test_late_arrival_uses_occurrence_time_not_logical_or_receipt(self):
        first = self.solo()
        late = dict(first, event_id="late", occurred_at=generator.timestamp(generator.ANCHOR + timedelta(days=1)),
                    recorded_at="2026-07-02T00:00:00Z", logical_ms="999999999999999999999999999")
        self.data["activity"] += [late]
        self.assertEqual(self.report()["retention"]["D1"]["numerator"], 1)


class NormalizationTests(unittest.TestCase):
    def setUp(self):
        self.signup = {"event_id": "same-event", "property": "identity", "visit_id": "auth-visit",
                       "kind": "signup", "authoritative": True, "campaign": "selected-campaign",
                       "occurred_at": "2026-01-01T02:00:00Z", "touch_id": "selected-touch"}
        self.visit = {"event_id": "same-event", "property": "portal", "visit_id": "landing-visit",
                      "kind": "visit", "campaign": "landing-campaign", "occurred_at": "2026-01-01T00:00:00Z",
                      "linkage_permitted": True}
        self.touch = {"touch_id": "selected-touch", "property": "portal", "visit_id": "landing-visit",
                      "campaign": "selected-campaign", "occurred_at": "2026-01-01T01:00:00Z",
                      "linkage_permitted": True}

    def normalize(self, visits=None, touches=None):
        return normalize_acquisition([self.signup], [self.visit] if visits is None else visits,
                                     [self.touch] if touches is None else touches)

    def test_exact_touch_to_visit_join_preserves_original_facts(self):
        rows, proof = self.normalize()
        linked = rows[0]
        self.assertEqual((linked["property"], linked["visit_id"]), ("identity", "auth-visit"))
        self.assertEqual((linked["attributed_visit_property"], linked["attributed_visit_id"]), ("portal", "landing-visit"))
        self.assertEqual(rows[1]["campaign"], "landing-campaign")
        self.assertEqual(proof["matched_signups"], 1)
        self.assertNotIn("attributed_visit_verified", self.signup)

    def test_multiple_touches_do_not_manufacture_visits(self):
        rows, proof = self.normalize(touches=[self.touch, dict(self.touch, touch_id="another-touch")])
        self.assertEqual(len(rows), 2)
        self.assertEqual(proof["matched_signups"], 1)

    def test_missing_disallowed_or_wrong_scope_cannot_link(self):
        for visits, touches in (([], [self.touch]), ([self.visit], []),
                                ([dict(self.visit, property="another-property")], [self.touch]),
                                ([dict(self.visit, linkage_permitted=False)], [self.touch]),
                                ([self.visit], [dict(self.touch, linkage_permitted=False)]),
                                ([self.visit], [dict(self.touch, visit_id=None)])):
            with self.subTest(visits=visits, touches=touches):
                rows, proof = self.normalize(visits, touches)
                self.assertNotIn("attributed_visit_verified", rows[0])
                self.assertEqual(proof["unresolved_selected_touches"], 1)

    def test_ambiguous_touch_and_inconsistent_clocks_or_campaign_fail(self):
        cases = [[self.touch, dict(self.touch, property="other")],
                 [dict(self.touch, occurred_at="2026-01-01T03:00:00Z")],
                 [dict(self.touch, occurred_at="2025-12-31T23:00:00Z")],
                 [dict(self.touch, campaign="wrong")]]
        for touches in cases:
            with self.subTest(touches=touches), self.assertRaises(ValueError):
                self.normalize(touches=touches)

    def test_duplicate_visits_and_touches_are_idempotent(self):
        rows, proof = self.normalize(visits=[self.visit, dict(self.visit)], touches=[self.touch, dict(self.touch)])
        self.assertEqual(len(rows), 2)
        self.assertEqual(proof["matched_signups"], 1)


class FileTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.path = Path(self.temp.name)
        generator.write_export(self.path)

    def load(self):
        return load_manifest(self.path / "manifest.json", CUTOFF)

    def mutate(self, filename, mutation):
        path = self.path / filename
        value = json.loads(path.read_text())
        mutation(value)
        path.write_text(json.dumps(value))

    def journey_bundle(self):
        rows = generator.manufacture()
        visits = [r for r in rows["acquisition"] if r["kind"] == "visit"]
        rows["acquisition"] = [r for r in rows["acquisition"] if r["kind"] != "visit"]
        touches = [dict(touch_id=r["event_id"], property=r["property"], visit_id=r["visit_id"],
                        campaign=r["campaign"], occurred_at=r["occurred_at"], linkage_permitted=r["linkage_permitted"])
                   for r in visits if r["campaign"] is not None]
        for row in rows["acquisition"]:
            row.update(property="identity", visit_id="auth-visit")
        rows["acquisition_visits"], rows["acquisition_touches"] = visits, touches
        generator.write_export(self.path, rows)

    def test_local_normalization_retains_all_original_manifest_references(self):
        self.journey_bundle()
        data, metadata, provenance = self.load()
        self.assertEqual(len(provenance["inputs"]), 6)
        self.assertEqual(provenance["normalization"]["matched_signups"], 11)
        self.assertEqual(campaign(calculate(data, metadata, CUTOFF))["signup_conversion"]["rate"], 9 / 10)
        for source in provenance["inputs"]:
            original = (self.path / source["manifest_file"]).read_bytes()
            self.assertEqual(source["manifest_sha256"], digest(original))
            self.assertEqual(source["manifest"], json.loads(original))
        output = self.path / "journey-report"
        subprocess.run([sys.executable, str(ROOT / "examples/analysis/analyze"), "--manifest", str(self.path / "manifest.json"),
                        "--cutoff", CUTOFF, "--output", str(output)], check=True, capture_output=True)
        report = json.loads((output / "campaign-report.json").read_text())
        self.assertEqual(campaign(report)["observed_converted_visits"], 10)
        self.assertEqual(report["provenance"]["normalization"]["matched_signups"], 11)
        self.assertNotIn("attributed_visit_verified", (self.path / "acquisition.jsonl").read_text())
        for row in data["acquisition"]:
            if row["kind"] == "signup":
                self.assertEqual((row["property"], row["visit_id"]), ("identity", "auth-visit"))

    def test_origin_export_incomplete_history_cannot_create_conclusive_rate(self):
        self.journey_bundle()
        self.mutate("acquisition_visits.manifest.json", lambda m: m["coverage"].update(history_start=None))
        data, metadata, _ = self.load()
        self.assertIsNone(campaign(calculate(data, metadata, CUTOFF))["signup_conversion"]["rate"])

    def test_origin_export_integrity_is_verified(self):
        self.journey_bundle()
        (self.path / "acquisition_touches.jsonl").write_text("{}\n")
        with self.assertRaisesRegex(ValueError, "integrity"):
            self.load()

    def test_reproducible_fixture_bytes(self):
        for expected in (FIXTURE_ROOT / "manufactured").iterdir():
            self.assertEqual(expected.read_bytes(), (self.path / expected.name).read_bytes())

    def test_manifest_transport_exact_fields(self):
        export_fields = {"export_id", "format_version", "status", "datasets", "query", "created_at", "expires_at",
                         "source_read_at", "watermark", "coverage", "continuation", "provenance"}
        dataset_fields = {"name", "file_name", "media_type", "bytes", "sha256", "row_count", "columns", "download_url", "ordering"}
        manifest = json.loads((self.path / "activity.manifest.json").read_text())
        self.assertEqual(set(manifest), export_fields)
        self.assertEqual(set(manifest["datasets"][0]), dataset_fields)

    def test_row_count_and_column_schema_mismatches_rejected(self):
        for mutation in (lambda m: m["datasets"][0].update(row_count=99999),
                         lambda m: m["datasets"][0].update(columns=[{"name": "wrong", "data_type": "string", "nullable": True}])):
            generator.write_export(self.path)
            self.mutate("activity.manifest.json", mutation)
            with self.assertRaises(ValueError):
                self.load()

    def test_tampering_rejected(self):
        with (self.path / "activity.jsonl").open("ab") as file:
            file.write(b"{}\n")
        with self.assertRaisesRegex(ValueError, "integrity"):
            self.load()

    def test_traversal_absolute_and_symlink_rejected(self):
        with tempfile.TemporaryDirectory() as outside:
            external = Path(outside) / "outside.jsonl"
            external.write_bytes((self.path / "activity.jsonl").read_bytes())
            (self.path / "link.jsonl").symlink_to(external)
            for value in (str(external), "../outside.jsonl", "link.jsonl"):
                self.mutate("activity.manifest.json", lambda m: m["datasets"][0].update(file_name=value))
                with self.assertRaises(ValueError):
                    self.load()

    def test_partial_continuation_sampled_and_missing_bounds_are_not_complete(self):
        for change in ({"status": "partial"}, {"continuation": {"cursor": "more"}},
                       {"coverage": {"complete": True, "history_start": None, "history_end": None,
                                     "sample_rate": None, "limitations": ["unavailable"], "policy_versions": [1]}}):
            generator.write_export(self.path)
            self.mutate("activity.manifest.json", lambda m: m.update(change))
            data, metadata, _ = self.load()
            self.assertIsNone(campaign(calculate(data, metadata, CUTOFF))["retention"]["D180"]["rate"])

    def test_failed_states_wrong_version_missing_coverage_and_future_cutoff(self):
        for change in ({"status": "failed"}, {"status": "cancelled"}, {"status": "expired"}, {"format_version": "future"}, {"coverage": {}}):
            generator.write_export(self.path)
            self.mutate("activity.manifest.json", lambda m: m.update(change))
            with self.assertRaises((ValueError, KeyError)):
                self.load()
        generator.write_export(self.path)
        with self.assertRaisesRegex(ValueError, "cutoff"):
            load_manifest(self.path / "manifest.json", "2027-01-01T00:00:00Z")

    def test_null_large_integer_and_provenance_survive(self):
        data, _, provenance = self.load()
        self.assertEqual(data["activity"][0]["logical_ms"], "9007199254740993")
        self.assertTrue(any(r["campaign"] is None for r in data["acquisition"]))
        self.assertEqual(provenance["manifest_sha256"], digest((self.path / "manifest.json").read_bytes()))
        for source in provenance["inputs"]:
            self.assertEqual(source["manifest"], json.loads((self.path / source["manifest_file"]).read_text()))

    def test_cli_outputs_are_byte_reproducible_and_aggregate(self):
        outputs = [self.path / "first", self.path / "second"]
        for output in outputs:
            subprocess.run([sys.executable, str(ROOT / "examples/analysis/analyze"), "--manifest", str(self.path / "manifest.json"),
                            "--cutoff", CUTOFF, "--output", str(output)], check=True, capture_output=True)
        for name in ("campaign-report.json", "campaign-report.txt"):
            self.assertEqual((outputs[0] / name).read_bytes(), (outputs[1] / name).read_bytes())
        report = json.loads((outputs[0] / "campaign-report.json").read_text())
        self.assertNotIn(generator.uid("subject/alpha"), json.dumps(report["campaigns"]))
        self.assertIn("D180: 1/3 (33.33%)", (outputs[0] / "campaign-report.txt").read_text())

    def test_cli_fails_closed_without_creating_output(self):
        self.mutate("activity.manifest.json", lambda m: m.update(status="failed"))
        output = self.path / "no-report"
        result = subprocess.run([sys.executable, str(ROOT / "examples/analysis/analyze"), "--manifest", str(self.path / "manifest.json"),
                                 "--cutoff", CUTOFF, "--output", str(output)], capture_output=True)
        self.assertEqual(result.returncode, 2)
        self.assertFalse(output.exists())

    def test_ambiguous_json_and_non_utc_rejected(self):
        for text in ('{"a":1,"a":2}', '{"a": NaN}'):
            with self.assertRaises(ValueError):
                parse_json(text)
        for value in ("2026-01-01", "2026-01-01T00:00:00", "2026-01-01T00:00:00-05:00"):
            with self.assertRaises(ValueError):
                utc(value)


if __name__ == "__main__":
    unittest.main()
