import copy
import importlib.util
import io
import json
import sys
import unittest
from decimal import Decimal, getcontext, localcontext

if not sys.flags.isolated:
    raise RuntimeError("Python must use -I")
spec = importlib.util.spec_from_file_location("candidate_solution", "/fixture/solution.py")
solution = importlib.util.module_from_spec(spec)
spec.loader.exec_module(solution)

class AcceptanceTests(unittest.TestCase):
    def test_empty(self):
        self.assertEqual(solution.dedupe_records([]), [])
        self.assertEqual(solution.dedupe_records(iter(())), [])

    def test_keep_first_and_order(self):
        rows = [{"id": 3, "v": "first3"}, {"id": 1, "v": "first1"},
                {"id": 3, "v": "last3"}, {"id": 2, "v": "first2"},
                {"id": 1, "v": "last1"}]
        self.assertEqual(solution.dedupe_records(rows), [rows[0], rows[1], rows[3]])

    def test_type_identity_and_boundary_ids(self):
        rows = [{"id": 1}, {"id": "1"}, {"id": ""}, {"id": 0},
                {"id": -7}, {"id": "中文"}, {"id": 0}, {"id": ""}]
        self.assertEqual(solution.dedupe_records(rows), rows[:6])

    def test_generator(self):
        seen = []
        def records():
            for value in [8, 2, 8, 9, 2]:
                seen.append(value)
                yield {"id": value, "position": len(seen)}
        self.assertEqual(solution.dedupe_records(records()),
                         [{"id": 8, "position": 1}, {"id": 2, "position": 2},
                          {"id": 9, "position": 4}])
        self.assertEqual(seen, [8, 2, 8, 9, 2])

    def test_shallow_copy_and_no_mutation(self):
        nested = {"labels": ["original"]}
        rows = [{"id": "a", "nested": nested}, {"id": "a", "other": 7}]
        before = copy.deepcopy(rows)
        result = solution.dedupe_records(rows)
        self.assertIsInstance(result, list)
        self.assertIsNot(result[0], rows[0])
        self.assertIs(result[0]["nested"], nested)
        result[0]["added"] = True
        self.assertEqual(rows, before)

    def test_repeated_calls_are_independent(self):
        first = solution.dedupe_records([{"id": 1, "value": "one"}])
        second = solution.dedupe_records([{"id": 1, "value": "two"}])
        self.assertEqual(second, [{"id": 1, "value": "two"}])
        self.assertIsNot(first, second)
        self.assertEqual(solution.dedupe_records([]), [])

    def test_many_records(self):
        rows = [{"id": index % 37, "position": index} for index in range(500)]
        self.assertEqual(solution.dedupe_records(rows), rows[:37])

    def test_invalid_input(self):
        for bad in [None, [], "x", 1]:
            with self.subTest(record=bad), self.assertRaises(TypeError):
                solution.dedupe_records([{"id": "valid"}, bad])
        for bad in [{}, {"id": None}, {"id": True}, {"id": False},
                    {"id": []}, {"id": {}}, {"id": 1.0}]:
            with self.subTest(record=bad), self.assertRaises(ValueError):
                solution.dedupe_records([{"id": 1}, bad])

if __name__ == "__main__":
    stream = io.StringIO()
    suite = unittest.defaultTestLoader.loadTestsFromTestCase(AcceptanceTests)
    result = unittest.TextTestRunner(stream=stream, verbosity=2).run(suite)
    print(stream.getvalue(), file=sys.stderr)
    print("WILLDEEP_ACCEPTANCE_JSON=" + json.dumps({
        "tests_run": result.testsRun,
        "failures": len(result.failures),
        "errors": len(result.errors),
        "skipped": len(result.skipped),
        "successful": result.wasSuccessful(),
    }, sort_keys=True))
    raise SystemExit(0 if result.wasSuccessful() else 1)
