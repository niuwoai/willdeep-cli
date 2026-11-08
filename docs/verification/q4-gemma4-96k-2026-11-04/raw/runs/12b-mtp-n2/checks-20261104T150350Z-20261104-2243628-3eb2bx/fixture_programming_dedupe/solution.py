def dedupe_records(records):
    seen_ids = set()
    result = []
    for record in records:
        if not isinstance(record, dict):
            raise TypeError("Each item must be a dictionary.")
        if 'id' not in record:
            raise ValueError("Record missing 'id' key.")
        
        rid = record['id']
        # Check if id is int (excluding bool) or str
        if isinstance(rid, bool) or (not isinstance(rid, (int, str))):
            raise ValueError("Invalid id type.")
        
        if rid not in seen_ids:
            seen_ids.add(rid)
            # Use shallow copy to avoid modifying original input
            result.append(record.copy())
    return result