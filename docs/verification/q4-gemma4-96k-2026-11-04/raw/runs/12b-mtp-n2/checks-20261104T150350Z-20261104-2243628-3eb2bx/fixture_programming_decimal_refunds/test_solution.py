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
    def check_totals(self, rows, expected):
        actual = solution.aggregate_amounts(rows)
        self.assertIsInstance(actual, dict)
        self.assertEqual(set(actual), set(expected))
        for currency, value in expected.items():
            with self.subTest(currency=currency):
                self.assertIsInstance(actual[currency], Decimal)
                self.assertEqual(actual[currency], Decimal(value))
                self.assertEqual(actual[currency].as_tuple().exponent, -2)

    def test_empty(self):
        self.check_totals([], {})
        self.check_totals(iter(()), {})

    def test_sales_and_refunds(self):
        self.check_totals([
            {"currency": "USD", "kind": "sale", "amount": "19.99"},
            {"currency": "USD", "kind": "refund", "amount": "2.34"},
            {"currency": "USD", "kind": "sale", "amount": "0.35"},
        ], {"USD": "18.00"})

    def test_multiple_currencies(self):
        self.check_totals([
            {"currency": "CNY", "kind": "sale", "amount": "10"},
            {"currency": "USD", "kind": "sale", "amount": "3.2"},
            {"currency": "EUR", "kind": "refund", "amount": "4.05"},
            {"currency": "CNY", "kind": "refund", "amount": "0.01"},
            {"currency": "USD", "kind": "refund", "amount": "0.2"},
        ], {"CNY": "9.99", "USD": "3.00", "EUR": "-4.05"})

    def test_fractional_accuracy(self):
        self.check_totals([
            {"currency": "USD", "kind": "sale", "amount": "0.01"}
            for _ in range(1000)
        ] + [{"currency": "USD", "kind": "refund", "amount": "9.99"}],
            {"USD": "0.01"})

    def test_refund_only_and_zero_balance(self):
        self.check_totals([
            {"currency": "USD", "kind": "refund", "amount": "1.01"},
            {"currency": "EUR", "kind": "sale", "amount": "7.77"},
            {"currency": "EUR", "kind": "refund", "amount": "7.77"},
            {"currency": "CNY", "kind": "sale", "amount": "0"},
        ], {"USD": "-1.01", "EUR": "0.00", "CNY": "0.00"})

    def test_generator_and_no_mutation(self):
        rows = [{"currency": "USD", "kind": "sale", "amount": "2.50", "extra": []},
                {"currency": "USD", "kind": "refund", "amount": "0.5"}]
        before = copy.deepcopy(rows)
        self.check_totals((row for row in rows), {"USD": "2.00"})
        self.assertEqual(rows, before)

    def test_large_values_and_decimal_context(self):
        with localcontext() as context:
            context.prec = 6
            before = context.copy()
            self.check_totals([
                {"currency": "USD", "kind": "sale", "amount": "123456789012345678901234567890.12"},
                {"currency": "USD", "kind": "sale", "amount": "0.01"},
                {"currency": "USD", "kind": "refund", "amount": "0.02"},
            ], {"USD": "123456789012345678901234567890.11"})
            self.assertEqual(context.prec, before.prec)
            self.assertEqual(context.rounding, before.rounding)
            self.assertEqual(context.traps, before.traps)
            self.assertEqual(context.flags, before.flags)

    def test_invalid_rows(self):
        valid = {"currency": "USD", "kind": "sale", "amount": "1.00"}
        for bad in [None, [], "row", 1]:
            with self.subTest(row=bad), self.assertRaises(TypeError):
                solution.aggregate_amounts([valid, bad])
        invalid = [{}, {"currency": "USD", "amount": "1"},
                   {"kind": "sale", "amount": "1"},
                   {"currency": "USD", "kind": "sale"}]
        for field, values in {
            "currency": ["usd", "US", "USDD", "美元", "ÜSD", None, 123],
            "kind": ["return", "SALE", "", None, True],
            "amount": ["-1", "+1", "01.00", ".5", "1.", "1.001", "1e2", "NaN",
                       "Infinity", " 1.00", "1.00 ", "", None, 1, 1.2, True],
        }.items():
            invalid.extend(dict(valid, **{field: value}) for value in values)
        for bad in invalid:
            with self.subTest(row=bad), self.assertRaises(ValueError):
                solution.aggregate_amounts([valid, bad])

    def test_repeated_calls_are_independent(self):
        self.check_totals([{"currency": "USD", "kind": "sale", "amount": "1"}], {"USD": "1.00"})
        self.check_totals([{"currency": "USD", "kind": "refund", "amount": "2"}], {"USD": "-2.00"})
        self.check_totals([], {})

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
