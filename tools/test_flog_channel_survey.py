#!/usr/bin/env python3
"""Tests for the FLog channel survey (#13). Run: python3 tools/test_flog_channel_survey.py"""

import json
import subprocess
import sys
import tempfile
import unittest
from collections import Counter
from pathlib import Path

HERE = Path(__file__).parent
sys.path.insert(0, str(HERE))
import flog_channel_survey as survey  # noqa: E402

SCRIPT = HERE / "flog_channel_survey.py"


class SurveyTest(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.dir = Path(self.tmp.name)
        # A stand-in for the engine: channel tags only appear inside format
        # strings, between other bytes.
        self.binary = self.dir / "lib.so"
        self.binary.write_bytes(
            b"\x00\x01[FLog::NativeDM] start\x00[DFLog::Net_Work] x {}\x00[FLog::NativeDM] again\x00"
            b"not a tag [FLog:: ] [Other::X]\x00"
        )
        self.settings = self.dir / "s.json"
        self.settings.write_text(json.dumps({"applicationSettings": {
            "FLogNativeDM": "Verbose", "DFLogNet_Work": "6", "FLogOnlyInSettings": "0", "FFlagX": "True"}}))

    def test_channels_come_from_format_string_prefixes(self):
        self.assertEqual(survey.channels_in_binary(self.binary), {"FLogNativeDM", "DFLogNet_Work"})

    def test_flags_cover_every_channel_with_one_shape(self):
        self.assertEqual(survey.flags_for({"FLogA", "DFLogB"}, "number", "7", "Verbose"),
                         {"DFLogB": "7", "FLogA": "7"})
        self.assertEqual(survey.flags_for({"FLogA"}, "severity", "7", "Verbose"), {"FLogA": "Verbose"})

    def test_line_counts_count_lines_not_occurrences(self):
        log = self.dir / "l.log"
        log.write_text("[FLog::NativeDM] a\n[FLog::NativeDM] b [FLog::NativeDM] c\n[DFLog::X] d\nplain\n")
        c = survey.count_lines(log)
        self.assertEqual((c["FLogNativeDM"], c["DFLogX"]), (2, 1))

    def test_verdicts(self):
        self.assertEqual(survey.verdict(3, 0, 0), "number-only")
        self.assertEqual(survey.verdict(0, 30, 29), "severity-only")
        self.assertEqual(survey.verdict(2, 2, 0), "either")
        self.assertEqual(survey.verdict(0, 0, 5), "silenced-by-both")
        self.assertEqual(survey.verdict(0, 0, 0), "unscored")

    def test_a_channel_cut_down_by_an_override_is_not_one_that_kept_talking(self):
        # 12 lines by default, 8 under a number: the number cost it lines.
        self.assertEqual(survey.verdict(8, 12, 12), "severity-only")
        self.assertEqual(survey.verdict(12, 12, 12), "either")

    def test_the_control_channel_is_scored_as_the_issue_measured_it(self):
        # #13's table: 29 lines absent, 0 with "100", 30 with "Verbose".
        rows = survey.score({"FLogNativeDM"}, {"FLogNativeDM": "severity"},
                            Counter(FLogNativeDM=29), [Counter()], Counter(FLogNativeDM=30))
        self.assertEqual(rows[0][5:], ("severity-only", "agrees"))

    def test_the_better_of_two_number_runs_is_the_one_scored(self):
        rows = survey.score({"FLogA"}, {}, Counter(), [Counter(), Counter(FLogA=3)], Counter(FLogA=3))
        self.assertEqual(rows[0][5], "either")

    def test_disagreement_with_the_settings_document_is_named(self):
        rows = survey.score({"FLogA"}, {"FLogA": "number"}, Counter(), [Counter()], Counter(FLogA=4))
        self.assertEqual(rows[0][6], "DISAGREES")
        self.assertIn("FLogA", survey.summary(rows))

    def test_a_filtered_settings_shape_still_compares_by_its_base(self):
        self.assertEqual(survey.agrees("severity-filtered", "severity-only"), "agrees")
        self.assertEqual(survey.agrees("severity", "either"), "-")
        self.assertEqual(survey.agrees("unknown", "severity"), "-")

    def test_cli_inventory_and_flags(self):
        out = subprocess.run([sys.executable, str(SCRIPT), "inventory", "--binary", str(self.binary),
                              "--settings", str(self.settings)], capture_output=True, text=True)
        self.assertEqual(out.returncode, 0, out.stderr)
        self.assertIn("FLogNativeDM\tseverity\tyes", out.stdout)
        self.assertIn("DFLogNet_Work\tnumber\tyes", out.stdout)
        self.assertIn("FLogOnlyInSettings\tnumber\tno", out.stdout)
        flags = subprocess.run([sys.executable, str(SCRIPT), "flags", "--shape", "severity",
                                "--binary", str(self.binary)], capture_output=True, text=True)
        self.assertEqual(json.loads(flags.stdout), {"DFLogNet_Work": "Verbose", "FLogNativeDM": "Verbose"})


if __name__ == "__main__":
    unittest.main()
