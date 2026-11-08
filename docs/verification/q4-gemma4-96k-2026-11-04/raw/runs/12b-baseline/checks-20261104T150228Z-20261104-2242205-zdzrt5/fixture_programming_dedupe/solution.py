def dedupe_records(records):
    seen_ids = set()
    result = []
    for record in records:
        if not isinstance(record, dict):
            raise TypeError("Each item must be a dictionary.")
        if 'id' not in record:
            raise ValueError("Record must contain an 'id' key.")
        
        val = record['id']
        # Check if id is int (excluding bool) or str
        if isinstance(val, bool) or (not isinstance(val, (int, str))):
            raise ValueError("ID must be an int or str.")
        
        if val not in seen_ids:
            seen_ids.add(val)
            # Use shallow copy to avoid modifying original input
            result.append(record.copy())
    return result