import decimal
from decimal import Decimal, Context, ROUND_HALF_UP
import re


def aggregate_amounts(rows):
    """
    Aggregates amounts by currency, handling sales and refunds with Decimal precision.
    """
    totals = {}
    # Regex for non-negative decimal string: (0|[1-9][0-9]*)(\.[0-9]{1,2})?
    # Must not have leading zeros (except 0.), signs, or scientific notation.
    pattern = re.compile(r'^(0|[1-9][0-9]*)(\.[0-9]{1,2})?$')

    for row in rows:
        if not isinstance(row, dict):
            raise TypeError("Each row must be a dictionary.")
        
        if not all(k in row for k in ('currency', 'kind', 'amount')):
            raise ValueError("Missing required fields.")
        
        currency = row['currency']
        kind = row['kind']
        amount_str = row['amount']

        if not isinstance(currency, str) or len(currency) != 3 or not currency.isupper() or not currency.isascii():
            raise ValueError("Invalid currency format.")
        
        if kind not in ('sale', 'refund'):
            raise ValueError("Invalid kind.")
        
        if not isinstance(amount_str, str) or not pattern.match(amount_str):
            raise ValueError("Invalid amount format.")
        
        # Convert to Decimal using the default context to avoid side effects
        # but ensure we handle the precision correctly.
        try:
            amount = Decimal(amount_str)
        except Exception:
            raise ValueError("Could not convert amount to Decimal.")

        if currency not in totals:
            totals[currency] = Decimal('0.00')
        
        if kind == 'sale':
            totals[currency] += amount
        else:
            totals[currency] -= amount

    # Final formatting: ensure every Decimal has exponent -2
    # We use a local context to avoid affecting the global one.
    result = {}
    for curr, val in totals.items():
        # Quantize to 2 decimal places using ROUND_HALF_UP
        # This ensures the exponent is -2.
        quantized = val.quantize(Decimal('0.01'), rounding=ROUND_HALF_UP)
        result[curr] = quantized
        
    return result