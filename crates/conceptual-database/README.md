# Conceptual database

A completely in-memory Rust database for learning how indexes change the work a
query performs. One `Order` table, three indexes, explicit HTTP queries, and atomic
batches. No SQL, persistence, background flushing, or external services.

```sh
cargo run --offline -p conceptual-database --example walkthrough
cargo run --offline -p conceptual-database -- --listen 127.0.0.1:3000
```

The walkthrough prints each access path, compares it with a forced scan, inspects
the indexes, and asserts that a failed batch leaves the database unchanged. The
HTTP server starts empty. Restarting loses all data. Use port `0` to allocate an
ephemeral port; the server prints its address. Ctrl-C stops it.

## One authoritative table, two indexes of IDs

```rust
struct Order {
    order_id: OrderId,
    customer_id: CustomerId,
    state: OrderState, // placed, shipped, cancelled
    product_id: ProductId,
    placed_at: u64,   // Unix seconds
}
```

| Index | Actual structure | Question it answers |
|---|---|---|
| Primary | `HashMap<OrderId, Order>` | Open order 101 |
| Secondary | `HashMap<OrderState, HashSet<OrderId>>` | Which orders await shipment? |
| Composite secondary | `BTreeMap<(CustomerId, u64), BTreeSet<OrderId>>` | What did customer 42 order recently? |

Only the primary map owns records. Secondary lookups find IDs and then fetch
records from the primary map. Multiple orders can share the same secondary key.
The composite index's ID sets are ordered to make timestamp ties deterministic.

Tuple keys sort by **customer first, then date**:

```text
(42, 100) -> {101}
(42, 200) -> {102}
(99, 300) -> {103}
```

Customer 42's history is contiguous. Dates across *all* customers are not.
Reversing that tuple would help global date searches but lose the contiguous
customer timeline. Index column order follows the questions you need to answer.

## Try the HTTP API

Seed three rows (the small timestamps stand in for calendar dates):

```sh
curl -sS localhost:3000/transactions -H 'Content-Type: application/json' -d '{
  "operations": [
    {"op":"create","order":{"order_id":101,"customer_id":42,"state":"placed","product_id":7,"placed_at":100}},
    {"op":"create","order":{"order_id":102,"customer_id":42,"state":"shipped","product_id":8,"placed_at":200}},
    {"op":"create","order":{"order_id":103,"customer_id":99,"state":"placed","product_id":7,"placed_at":300}}
  ]
}'

# Primary lookup: fetch exactly one record.
curl -sS localhost:3000/orders/101

# State lookup: visit one bucket and fetch two records.
curl -sS 'localhost:3000/orders?state=placed'

# Composite range: inclusive endpoints, visits two buckets and records.
curl -sS 'localhost:3000/orders?customer_id=42&from=100&through=200'

# Reverse scan: stop after the first matching record.
curl -sS 'localhost:3000/orders?customer_id=42&latest=1'

# Composite candidates plus a residual state filter: examine two, return one.
curl -sS 'localhost:3000/orders?customer_id=42&state=placed'

# Same result, but examine all three primary records before sorting/limiting.
curl -sS 'localhost:3000/orders?customer_id=42&latest=1&force_scan=true'

# Both need full scans: no product index and no leading customer predicate.
curl -sS 'localhost:3000/orders?product_id=7'
curl -sS 'localhost:3000/orders?from=100&through=200'
```

A query response includes the orders and its execution report:

```json
{
  "orders": [{"order_id":101,"customer_id":42,"state":"placed","product_id":7,"placed_at":100}],
  "execution": {
    "plan":"customer_date_range",
    "index_keys_visited":2,
    "records_examined":2,
    "records_returned":1,
    "residual_filters":["state"],
    "sort_required":false
  }
}
```

`index_keys_visited` counts successful index buckets visited, not B-tree comparison
steps, hash probes, or individual IDs. Full scans report zero index buckets.
`records_examined` counts primary records fetched, including candidates rejected
by filters. These are logical work counts, not timings or CPU cost estimates.
`sort_required` describes the plan, even if a particular result is empty.

The planner is deliberately explicit: `force_scan` wins; otherwise customer uses
the composite index, state uses the state index, and everything else scans.
Other predicates are residual filters. This is not a cost-based optimizer:
a rare state might be more selective than a large customer history, and an index
on a very common state can save little work.

All supplied filters are ANDed. Omitted date bounds mean `0` and `u64::MAX`.
`from > through` is an error. `latest` must be positive and requires a customer.
Customer queries sort by `(placed_at, order_id)` ascending, or descending for
`latest`. Other queries sort by order ID for repeatable output; the hash-based
state index does not provide that ordering. Forced scans return the same order.
With residual filters, a latest query stops after enough **matches**, not after
that many candidates.

## See writes maintain the indexes

```sh
curl -sS localhost:3000/debug/indexes
curl -i -X PUT localhost:3000/orders/101 -H 'Content-Type: application/json' \
  -d '{"order_id":101,"customer_id":42,"state":"shipped","product_id":7,"placed_at":100}'
curl -sS localhost:3000/debug/indexes
curl -i -X DELETE localhost:3000/orders/101
curl -sS localhost:3000/debug/indexes
curl -sS localhost:3000/debug/check
```

Replacement removes old secondary entries and inserts new ones, even when some
indexed fields are unchanged. Deletion removes all entries and empty buckets.
This is write amplification: one logical write maintains several structures and
uses extra memory. The checker rebuilds secondary indexes from the primary table
and compares exactly, detecting missing, stale, dangling, and empty entries.
Its unit test deliberately corrupts indexes; the HTTP API exposes only valid writes.

| Endpoint | Behavior |
|---|---|
| `POST /orders` | Create, `201`; duplicate ID is `409` |
| `GET /orders/{id}` | Primary lookup with execution report; missing is `404` |
| `PUT /orders/{id}` | Replace existing, `204`; body ID must match path |
| `DELETE /orders/{id}` | Delete existing, `204`; missing is `404` |
| `GET /orders?...` | Query with execution report |
| `POST /transactions` | Ordered batch of `create`, `replace`, `delete` |
| `GET /debug/indexes` | Sorted snapshot of primary records and secondary buckets |
| `GET /debug/check` | `{"consistent":true}` or `500` with an invariant error |

Unknown query/body fields and malformed values are rejected. Domain/query errors
return JSON; Axum extractor errors may return text with `400`, `415`, or `422`.

## Locking is isolation; staging provides rollback

Every HTTP operation uses the same `Arc<tokio::sync::RwLock<Database>>`.
Concurrent readers share a lock; a writer excludes all other readers and writers.
Existence checks and writes happen under that lock. Reads copy their results and
release the lock before serializing the HTTP response. There is no network or
other awaited work inside a locked database operation.

Individual writes validate before mutating, then maintain all indexes. The lock
prevents readers from seeing intermediate index changes; it does not roll back
a panic in an individual mutation.

A batch holds the exclusive lock, clones the database, applies operations in
request order to the clone, and swaps it into place only if all succeed. Later
operations see earlier changes within that batch. On an error, the clone is
discarded. An empty batch succeeds with `applied: 0`.

Try this after seeding a fresh server. The second create conflicts, so order 104
never appears:

```sh
curl -i localhost:3000/transactions -H 'Content-Type: application/json' -d '{
  "operations": [
    {"op":"create","order":{"order_id":104,"customer_id":42,"state":"placed","product_id":7,"placed_at":400}},
    {"op":"create","order":{"order_id":102,"customer_id":42,"state":"placed","product_id":7,"placed_at":500}}
  ]
}'
curl -i localhost:3000/orders/104
curl -sS localhost:3000/debug/check
```

Change the second ID to 105 and repeat: both creates commit. Cloning uses time and
memory proportional to the entire database and blocks readers throughout the
batch. This teaches atomicity and isolation without undo logs or MVCC. Separate
HTTP requests do not share a transaction. There is no durability and no rollback
of effects outside this database. Fine-grained locks, deadlocks, snapshot reads,
and recovery are later experiments.

## What these indexes buy

With N orders and K candidates, expected primary hash lookup is O(1); state
selection and fetching is O(1 + K), plus result sorting. Composite range access
is O(log B + K), where B is the number of customer/date buckets. Latest scans can
terminate early. Non-customer result ordering costs O(R log R) for R returned
records; forced customer scans also sort before limiting. A full scan examines
N records. Secondary writes update hash sets and ordered buckets/ID sets.

An index that does not support a predicate does not prevent answering it: it
means scanning, residual filtering, or extra sorting. No covering index, index
intersection, automatic index selection, joins, or pagination is implemented.

This is a single-table database teaching relational storage ideas. An HTTP API
does not prevent a relational model, but this example has no relationships or
foreign-key constraints. Adding a Customer table and enforcing its references
would be a separate lesson.

## Verify

```sh
cargo fmt -p conceptual-database -- --check
cargo test --offline -p conceptual-database
cargo clippy --offline -p conceptual-database --all-targets -- -D warnings
```

Tests compare indexed and scan results across filter combinations, boundary
timestamps, ties, and limits; assert work counts; check index movement and batch
rollback; exercise competing writers; and launch the actual HTTP binary on an
ephemeral loopback port for CRUD, validation, transactions, and empty startup.

Start reading [database.rs](src/database.rs) for ownership and writes,
[query.rs](src/query.rs) for access paths, and [http.rs](src/http.rs) for lock scope.
