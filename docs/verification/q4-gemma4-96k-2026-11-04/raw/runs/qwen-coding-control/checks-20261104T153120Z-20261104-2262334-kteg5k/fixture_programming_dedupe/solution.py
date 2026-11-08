def dedupe_records(records):
    seen = set()
    result = []
    for record in records:
        if not isinstance(record, dict):
            raise TypeError("Each record must be a dict")
        if 'id' not in record:
            raise ValueError("Record missing 'id'")
        rid = record['id']
        if isinstance(rid, bool):
            raise ValueError("id must be int or str, not bool")
        if not isinstance(rid, (int, str)):
            raise ValueError("id must be int or str")
        if isinstance(rid, str) and rid == "":
            raise ValueError("id cannot be empty string")
        if isinstance(rid, int) and rid == 0:
            raise ValueError("id cannot be 0")
        if isinstance(rid, int) and rid < 0:
            raise ValueError("id cannot be negative")
        if rid in seen:
            continue
        seen.add(rid)
        result.append(dict(record))
    return result
