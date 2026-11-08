import re
from decimal import Decimal, localcontext, ROUND_HALF_UP

def aggregate_amounts(rows):
    """
    Aggregates amounts by currency, handling sales and refunds with Decimal precision.
    """
    totals = {}
    # Regex for non-negative decimal string: (0|[1-9][0-9]*)(\.([0-9]{1,2}))?
    # We need to ensure no leading zeros (except 0.xx), no signs, no scientific notation.
    pattern = re.compile(r'^(0|\d{1,})(\.\d{1,2})?$')
    
    for row in rows:
        if not isinstance(row, dict):
            raise TypeError("Row must be a dictionary.")
        
        if not all(k in row for k in ('currency', 'kind', 'amount')):
            raise ValueError("Missing required fields.")
        
        currency = row['currency']
        kind = row['kind']
        amount_str = row['amount']
        
        if not isinstance(currency, str) or len(currency) != 3 or not currency.isupper() or not currency.isascii():
            raise ValueError("Invalid currency format.")
        
        if kind not in ('sale', 'refund'):
            raise ValueError("Kind must be 'sale' or 'refund'.")
        
        if not isinstance(amount_str, str) or not pattern.match(amount_str):
            raise ValueError("Invalid amount format.")
        
        # Convert to Decimal
        # We use a localcontext to ensure precision doesn't affect the global state
        # and to force the rounding/quantization behavior.
        with localcontext() as ctx:
            # Decimal(amount_str) is safe here because we validated the string format
            val = Decimal(amount_str)
            if val < 0: # Should not happen due to regex but for safety
                raise ValueError("Amount must be non-negative.")
            
            if kind == 'sale':
                totals[currency] = totals.get(currency, Decimal('0.00')) + val
            else:
                totals[currency] = totals.get(currency, Decimal('0.00')) - val

    # Final quantization to 2 decimal places
    result = {}
    for curr, total in totals.items():
        # Quantize to 2 decimal places using ROUND_HALF_UP
        # We use '0.00' to ensure exponent is -2
        result[curr] = total.quantize(Decimal('0.00'), rounding=ROUND_HALF_UP)
        
    return result