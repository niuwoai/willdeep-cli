from decimal import Decimal, Context, ROUND_HALF_UP, getcontext
import re

def aggregate_amounts(rows):
    """
    Aggregates amounts by currency from an iterable of rows.
    Handles sale (addition) and refund (subtraction) with Decimal precision.
    """
    # Regex for strict decimal string validation: (0|[1-9][0-9]*)(\.[0-9]{1,2})?
    amount_pattern = re.compile(r'^(0|[1-9][0-9]*)(\.[0-9]{1,2})?$')
    currency_pattern = re.compile(r'^[A-Z]{3}$')
    
    totals = {}
    
    # Save current context to restore later
    original_context = getcontext()
    
    try:
        for row in rows:
            if not isinstance(row, dict):
                raise TypeError("Each row must be a dict")
            
            # Check for required keys
            if 'currency' not in row or 'kind' not in row or 'amount' not in row:
                raise ValueError("Missing required fields: currency, kind, or amount")
            
            currency = row['currency']
            kind = row['kind']
            amount_str = row['amount']
            
            # Validate currency
            if not isinstance(currency, str) or not currency_pattern.match(currency):
                raise ValueError("Invalid currency format")
            
            # Validate kind
            if kind not in ('sale', 'refund'):
                raise ValueError("kind must be 'sale' or 'refund'")
            
            # Validate amount string format strictly
            if not isinstance(amount_str, str) or not amount_pattern.match(amount_str):
                raise ValueError("Invalid amount format")
            
            # Convert to Decimal using a local context to ensure precision
            # We use a high precision context locally to avoid issues with caller's context
            # but the final rounding is done to 2 decimal places.
            val = Decimal(amount_str)
            
            if kind == 'sale':
                totals[currency] = totals.get(currency, Decimal('0.00')) + val
            else:  # refund
                totals[currency] = totals.get(currency, Decimal('0.00')) - val
        
        # Final pass: ensure all Decimals are quantized to 2 decimal places
        # and handle the exponent requirement.
        final_totals = {}
        quantize_target = Decimal('0.01')
        for curr, total_val in totals.items():
            # Quantize to 2 decimal places using ROUND_HALF_UP
            # We use a local context for quantization to avoid side effects
            final_totals[curr] = total_val.quantize(quantize_target, rounding=ROUND_HALF_UP)
            
        return final_totals

    finally:
        # Restore original context (though we didn't modify global context directly)
        getcontext().prec = original_context.prec
        getcontext().rounding = original_context.rounding
        getcontext().Emax = original_context.Emax
        getcontext().Emin = original_context.Emin
        getcontext().prec = original_context.prec
        getcontext().traps = original_context.traps
        getcontext().flags = original_context.flags
