def dedupe_records(records):
    seen = set()
    result = []
    for record in records:
        if not isinstance(record, dict):
            raise TypeError("record must be a dict")
        if 'id' not in record:
            raise ValueError("record missing 'id'")
        rid = record['id']
        if isinstance(rid, bool) or not isinstance(rid, (int, str)):
            raise ValueError("invalid id type")
        if rid not in seen:
            seen.add(rid)
            result.append(dict(record))
    return result
