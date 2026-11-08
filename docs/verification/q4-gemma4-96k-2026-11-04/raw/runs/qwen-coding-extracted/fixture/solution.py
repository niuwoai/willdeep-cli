from decimal import Decimal, getcontext, ROUND_HALF_EVEN
import re

def aggregate_amounts(rows):
    # Pattern for amount: non-negative decimal string with up to 2 decimal places
    # Strictly matches (0|[1-9][0-9]*)(\.[0-9]{1,2})?
    amount_pattern = re.compile(r'^(0|[1-9][0-9]*)(\.[0-9]{1,2})?$')
    
    totals = {}
    
    for row in rows:
        if not isinstance(row, dict):
            raise TypeError("Each row must be a dict")
        
        # Check required fields
        if 'currency' not in row or 'kind' not in row or 'amount' not in row:
            raise ValueError("Missing required fields")
        
        currency = row['currency']
        kind = row['kind']
        amount_str = row['amount']
        
        # Validate currency: exactly three ASCII uppercase letters
        if not isinstance(currency, str) or len(currency) != 3 or not currency.isupper() or not currency.isalpha():
            raise ValueError("Invalid currency")
        
        # Validate kind: must be 'sale' or 'refund'
        if kind not in ('sale', 'refund'):
            raise ValueError("Invalid kind")
        
        # Validate amount: must be a string matching the pattern
        if not isinstance(amount_str, str):
            raise ValueError("Invalid amount type")
        
        if not amount_pattern.match(amount_str):
            raise ValueError("Invalid amount format")
        
        # Convert to Decimal
        amount = Decimal(amount_str)
        
        # Initialize totals for this currency if not present
        if currency not in totals:
            totals[currency] = Decimal('0.00')
        
        if kind == 'sale':
            totals[currency] += amount
        elif kind == 'refund':
            totals[currency] -= amount
    
    # Round all totals to 2 decimal places and ensure they are Decimal with exponent -2
    result = {}
    for currency, total in totals.items():
        # Quantize to 2 decimal places
        rounded = total.quantize(Decimal('0.01'), rounding=ROUND_HALF_EVEN)
        result[currency] = rounded
    
    return result
