def dedupe_records(records):
    seen_ids = set()
    result = []
    
    for record in records:
        if not isinstance(record, dict):
            raise TypeError("Each record must be a dict")
        
        if 'id' not in record:
            raise ValueError("Record missing 'id' key")
        
        record_id = record['id']
        
        # Validate id type: int or str, but not bool
        if isinstance(record_id, bool):
            raise ValueError("ID cannot be a boolean")
        if not isinstance(record_id, (int, str)):
            raise ValueError("ID must be an int or a str")
        
        # Check for duplicates
        if record_id not in seen_ids:
            seen_ids.add(record_id)
            # Append a shallow copy to avoid mutating input
            result.append(record.copy())
            
    return result