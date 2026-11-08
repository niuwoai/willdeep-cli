import re
from decimal import Decimal, ROUND_HALF_UP, localcontext

_AMOUNT_RE = re.compile(r'^(0|[1-9][0-9]*)(\.[0-9]{1,2})?$')

def aggregate_amounts(rows):
    totals = {}
    for row in rows:
        if not isinstance(row, dict):
            raise TypeError('row must be a dict')
        if 'currency' not in row or 'kind' not in row or 'amount' not in row:
            raise ValueError('missing required field')
        currency = row['currency']
        kind = row['kind']
        amount = row['amount']
        if not isinstance(currency, str) or len(currency) != 3 or not all('A' <= c <= 'Z' for c in currency):
            raise ValueError('invalid currency')
        if kind not in ('sale', 'refund'):
            raise ValueError('invalid kind')
        if not isinstance(amount, str):
            raise ValueError('amount must be a string')
        if not _AMOUNT_RE.match(amount):
            raise ValueError('invalid amount format')
        with localcontext() as ctx:
            ctx.prec = 50
            ctx.rounding = ROUND_HALF_UP
            dec = Decimal(amount)
            dec = dec.quantize(Decimal('0.01'), rounding=ROUND_HALF_UP)
            if kind == 'sale':
                totals[currency] = totals.get(currency, Decimal('0.00')) + dec
            else:
                totals[currency] = totals.get(currency, Decimal('0.00')) - dec
    for cur in totals:
        totals[cur] = totals[cur].quantize(Decimal('0.01'), rounding=ROUND_HALF_UP)
    return totals
