{
  memory = {
    enabled = true,
    estimated_items_capacity = 4096,
    weight_capacity = 67108864,
    shards = null,
    hot_allocation = 0.97,
    ghost_allocation = 0.5,
    max_entry_weight = 8388608,
  },
  disk = {
    io_backoff = {
      initial_delay_ms = 2,
      max_delay_ms = 64,
      max_elapsed_ms = 1500,
      jitter = true,
    },
    prune_trigger_bytes = 4294967296,
    prune_older_than_secs = 604800,
    prune_on_write = true,
    directories = true,
    busy_timeout_ms = 5000,
    sqlite_cache_kib = 8192,
    wal_autocheckpoint_pages = 1000,
  },
}
