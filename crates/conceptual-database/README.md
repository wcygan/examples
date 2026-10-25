# Conceptual database

A completely in-memory Rust database for learning how indexes change the work a
query performs. One `Order` table, five indexes, explicit HTTP queries, and atomic
batches. The secondary indexes run on std's B-tree or on
[bbtree](https://github.com/XiangpengHao/bbtree), and a live database can
[swap between them](#swap-the-b-tree-std-or-bbtree): 1.5–1.8× faster index
builds and intersections, [up to 11× on bare-tree scans](#how-much-faster-is-bbtree). No SQL, persistence,
background flushing, or external services.

```sh
cargo run --offline -p conceptual-database --example walkthrough
cargo run --offline -p conceptual-database -- --listen 127.0.0.1:3000
cargo run --release --offline -p conceptual-database --example index_strategies
cargo run --release --offline -p conceptual-database --example tree_swap
```

The walkthrough prints each access path, compares it with a forced scan, inspects
the indexes, and asserts that a failed batch leaves the database unchanged. The
HTTP server starts empty. Restarting loses all data. Use port `0` to allocate an
ephemeral port; the server prints its address. Ctrl-C stops it.

## One authoritative table, four indexes of IDs

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
| Secondary | `HashMap<OrderState, BTreeSet<OrderId>>` | Which orders await shipment? |
| Secondary | `HashMap<ProductId, BTreeSet<OrderId>>` | Who ordered product 7? |
| Composite secondary | `BTreeSet<(CustomerId, u64, OrderId)>` | What did customer 42 order recently? |
| Composite secondary | `HashMap<(ProductId, OrderState), BTreeSet<OrderId>>` | Which product 7 orders were cancelled? |

Only the primary map owns records. As in InnoDB, the IDs inside one bucket are
kept in primary-key order; [index intersection](#one-index-two-or-a-composite)
depends on that. Secondary lookups find IDs and then fetch
records from the primary map. Multiple orders can share the same secondary key.
Every `BTreeSet` above is std's or bbtree's `BBTreeSet`; see [Swap the
B-tree](#swap-the-b-tree-std-or-bbtree).

The customer/date index is one sorted set, not a map of ID sets. Like an InnoDB
secondary index record, each entry is the key columns followed by the primary
key, so entries are unique and timestamp ties sort by order ID. Entries sort by
**customer first, then date**:

```text
(42, 100, 101)
(42, 200, 102)
(99, 300, 103)
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

# Product index: one bucket.
curl -sS 'localhost:3000/orders?product_id=7'

# Product and state: the planner uses the composite; hints force the others.
curl -sS 'localhost:3000/orders?product_id=7&state=placed'
curl -sS 'localhost:3000/orders?product_id=7&state=placed&index=intersect_product_state'
curl -sS 'localhost:3000/orders?product_id=7&state=placed&index=product'

# Full scan: no index leads with the date.
curl -sS 'localhost:3000/orders?from=100&through=200'
```

A query response includes the orders and its execution report:

```json
{
  "orders": [{"order_id":101,"customer_id":42,"state":"placed","product_id":7,"placed_at":100}],
  "execution": {
    "plan":"customer_date_range",
    "index_keys_visited":2,
    "index_entries_read":2,
    "records_examined":2,
    "records_returned":1,
    "residual_filters":["state"],
    "sort_required":false
  }
}
```

`index_keys_visited` counts successful index buckets visited, not B-tree comparison
steps or hash probes. `index_entries_read` counts the IDs read from those buckets.
Full scans report zero for both.
`records_examined` counts primary records fetched, including candidates rejected
by filters. These are logical work counts, not timings or CPU cost estimates.
`sort_required` describes the plan, even if a particular result is empty.

The planner is deliberately explicit: `force_scan` wins; otherwise customer uses
the customer/date index, product plus state uses the product/state index, then
product alone, then state alone, and everything else scans. `index` overrides the
planner like MySQL `FORCE INDEX`: `state`, `product`, `customer_date`,
`product_state`, or `intersect_product_state`. A hint needs the predicates its
index covers and cannot be combined with `force_scan`. Other predicates are
residual filters. This is not a cost-based optimizer:
a rare state might be more selective than a large customer history, and an index
on a very common state can save little work.

All supplied filters are ANDed. Omitted date bounds mean `0` and `u64::MAX`.
`from > through` is an error. `latest` must be positive and requires a customer.
Customer queries sort by `(placed_at, order_id)` ascending, or descending for
`latest`. Other queries return order-ID order, which the product and state
buckets already provide; scans and customer queries via another index sort.
Forced scans return the same order.
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
is O(log N + K). Latest scans can
terminate early. Non-customer result ordering costs O(R log R) for R returned
records; forced customer scans also sort before limiting. A full scan examines
N records. Secondary writes update a hash bucket's ordered ID set, or one entry
of the customer/date set.

An index that does not support a predicate does not prevent answering it: it
means scanning, residual filtering, or extra sorting. No covering index, index
union, cost-based index selection, joins, or pagination is implemented.

## One index, two, or a composite?

For `product_id = ? AND state = ?` there are four indexed strategies:

1. **One index, then filter.** Read one index's IDs, fetch every row, and discard
   rows with the wrong value in the other column.
2. **Index intersection.** Read *both* ID lists, merge them (both are in ID order),
   and fetch only the IDs in both. This is MySQL's index merge: `EXPLAIN` shows
   `type: index_merge` and `Using intersect(idx_product,idx_state)`.
3. **Composite index.** One bucket holds exactly the matching IDs.
4. **Full scan.** Fetch everything.

The `index_strategies` example loads 1M orders: 1,000 products with 10% of orders
on one hot product 0, and 90% shipped, 8% placed, 2% cancelled. It runs each
strategy, asserts identical results, and reports work and median time. One run on
an Apple Silicon laptop:

| `product = 7 AND state = cancelled` | index reads | rows fetched | median |
|---|---:|---:|---:|
| full scan | 0 | 1,000,000 | 2.2 ms |
| state index + filter | 20,087 | 20,087 | 224 µs |
| product index + filter | 886 | 886 | 7.0 µs |
| intersect product ∩ state | 20,962 | 24 | 58.8 µs |
| composite (product, state) | 24 | 24 | 0.3 µs |

| `product = 0 AND state = cancelled` | index reads | rows fetched | median |
|---|---:|---:|---:|
| full scan | 0 | 1,000,000 | 2.2 ms |
| state index + filter | 20,087 | 20,087 | 263 µs |
| product index + filter | 100,191 | 100,191 | 1.8 ms |
| intersect product ∩ state | 120,270 | 2,071 | 483 µs |
| composite (product, state) | 2,071 | 2,071 | 20.5 µs |

For `product = 7 AND state = shipped`, the state index fetches 900,012 rows. It
takes 27 ms, about nine times slower than the 2.9 ms full scan. Random lookups in
the primary table cost more than walking it sequentially. MySQL also prefers a
table scan over a low-selectivity index for this reason.

What the numbers show:

- **The composite index always wins.** It reads and fetches only the answer.
  You pay for it on every write and in memory: one more index to maintain.
- **Intersection fetches the fewest rows of any plan except the composite, but it
  reads every entry in both lists** until one list ends. It pays off only when
  each list alone would fetch many rows and the overlap is small, as with the hot
  product. Against a selective index (product 7), filtering is cheaper.
- **Selectivity, not index count, decides.** An index on a common value (shipped)
  can be worse than no index.
- In memory, reading an index entry costs only a few times less than fetching a
  row. In InnoDB a row fetch is a clustered B-tree lookup and possibly disk I/O,
  so intersection's trade of more index reads for fewer fetches favors it more there.

### The same experiment in MySQL 8.4

[`mysql/index_strategies.sql`](mysql/index_strategies.sql) builds the same table
in a throwaway container. It forces each strategy with hints (`FORCE INDEX`,
`INDEX_MERGE`, `NO_INDEX`) and prints `EXPLAIN ANALYZE`:

```sh
docker run --rm -d --name cdb-mysql -e MYSQL_ALLOW_EMPTY_PASSWORD=yes -e MYSQL_DATABASE=lab mysql:8.4
docker exec -i cdb-mysql mysql -t lab < crates/conceptual-database/mysql/index_strategies.sql
docker stop cdb-mysql
```

One run, actual times from `EXPLAIN ANALYZE` with all data in the buffer pool:

| Strategy | product 7, cancelled | product 0, cancelled |
|---|---:|---:|
| full scan | 230 ms | — |
| `FORCE INDEX (idx_state)` | 14.4 ms | 11.0 ms |
| `FORCE INDEX (idx_product)` | 0.8 ms | 41.5 ms |
| `INDEX_MERGE(idx_product, idx_state)` | 1.9 ms | 11.4 ms |
| `FORCE INDEX (idx_product_state)` | 0.02 ms | 1.7 ms |

The pattern matches the in-memory one. With every index available, MySQL's
optimizer picks `idx_product_state` without hints. With that index hidden by
`NO_INDEX(orders idx_product_state)`, it chooses `Using intersect(...)` on its
own. MySQL intersects secondary indexes only when every key part is compared
with equality. Only then are the matching entries in primary-key order. A
`customer_id = ? AND placed_at > ?` range on `idx_customer_date` does not
qualify, and MySQL falls back to a single index.

This is a single-table database teaching relational storage ideas. An HTTP API
does not prevent a relational model, but this example has no relationships or
foreign-key constraints. Adding a Customer table and enforcing its references
would be a separate lesson.

## Swap the B-tree: std or bbtree

[bbtree](https://bbtree.xiangpeng.systems/post/) is a B-tree whose
`unsync::BBTreeMap`/`BBTreeSet` are drop-in replacements for std's. Its nodes
keep a short big-endian prefix of each key ("inline key") beside the keys and
search those first. `OrderId` and the customer/date entry implement bbtree's
`InlineKey` so they get one; without it a newtype or tuple key gets none.

[`tree.rs`](src/tree.rs) wraps both trees in one enum per structure, and a
`Database` picks one `Tree` for all its indexes. `set_tree` rebuilds every
secondary index on the other tree from the primary records, like an offline
index rebuild: same rows, same plans, same work counts, different structures.

```sh
cargo run --offline -p conceptual-database -- --listen 127.0.0.1:3000 --tree bbtree

curl -sS localhost:3000/debug/tree
# Rebuild under the write lock; queries wait, then report "tree": "std".
curl -sS -X PUT localhost:3000/debug/tree -H 'content-type: application/json' -d '{"tree":"std"}'
```

Every query's `execution` and `/debug/indexes` name the tree. `/debug/check`
also fails if any index is on a different tree than the database.

### How much faster is bbtree?

**On its own, bbtree is about 2.4× faster at lookups and 11× faster at full
iteration than std's B-tree. Inside this database, swapping it in makes index
builds and index intersections 1.5–1.8× faster. Everything else stays within
noise of std**, because those queries spend their time fetching records from
the primary `HashMap`, which neither tree touches. bbtree's own README reports
up to 5× on its benchmarks; the numbers below are what this machine measures.

`tree_swap` measures both levels. Each cell is the median of three runs on an
Apple Silicon laptop; "speedup" is std time ÷ bbtree time, so above 1× means
bbtree is faster, and the last column is the spread across the three runs.

**Tree only.** 1M random `u64` keys in a bare `BTreeSet` vs `BBTreeSet`, each
operation applied to every key in random order:

| Operation | std | bbtree | speedup | per-run |
|---|---:|---:|---:|---:|
| full scan (iterate all) | 4.5 ms | 0.40 ms | **11.4×** | 10.7–12.6× |
| lookup (`contains`) | 89.5 ms | 36.7 ms | **2.4×** | 2.3–2.6× |
| range (next 10 keys) | 109.1 ms | 66.9 ms | **1.6×** | 1.5–1.9× |
| insert | 86.8 ms | 70.9 ms | 1.2× | 1.1–1.2× |
| remove | 100.1 ms | 107.3 ms | 0.9× | 0.9–1.0× |

**In the database.** The same 1M orders and 4 secondary indexes on each tree;
every query asserts identical rows on both:

| Workload | std | bbtree | speedup | per-run |
|---|---:|---:|---:|---:|
| load (1M creates, 4 index inserts each) | 482.8 ms | 269.4 ms | **1.8×** | 1.5–1.8× |
| intersection (120,270 IDs read, 2,071 fetched) | 735.5 µs | 413.0 µs | **1.8×** | 1.6–1.8× |
| 10k customers, latest 3 | 6.1 ms | 4.9 ms | 1.2× | 0.9–1.4× |
| composite bucket (2,071 IDs + fetches) | 25.9 µs | 24.7 µs | 1.0× | 1.0–1.1× |
| 10k customers, date range | 4.9 ms | 5.1 ms | 1.0× | 0.9–1.1× |
| 100k replaces | 176.5 ms | 195.4 ms | 0.9× | 0.8–1.0× |
| rebuild all indexes on the other tree | 557 ms (to std) | 336 ms (to bbtree) | | |

Why the database sees less than the bare tree:

- **bbtree wins where the tree is the work.** Intersection merges two long ID
  lists, which is iteration, bbtree's strongest operation. Loading is 4M index
  inserts and gains more than the random-order bare insert (1.2×). Most IDs
  arrive in ascending order, which probably favors bbtree, but this benchmark
  doesn't isolate that.
- **The primary `HashMap` does the rest.** The composite bucket, customer scans
  and replaces spend most of their time hashing, fetching and cloning `Order`
  records. Speeding up the index can't speed that part up.
- **No query here is a point lookup on a secondary tree**, bbtree's 2.4× case.
  Secondary indexes are probed through `HashMap` buckets, or entered once per
  range scan.
- Short customer scans read 3 to 5 entries, so they are mostly one descent to the
  start, and their timings vary run to run.

### Shape the index for the tree

The customer/date index first held a `BTreeMap<(CustomerId, u64), BTreeSet<OrderId>>`:
a set per timestamp, and almost every set holds one order. On bbtree, each of
those ~1M one-element sets allocated its own leaf page, 1 KiB by default:

| Customer/date index | peak RSS, std | peak RSS, bbtree |
|---|---:|---:|
| map of one-element ID sets | 474 MB | 2,387 MB |
| one set of `(customer, date, order)` entries | 317 MB | 295 MB |

The nested version also made bbtree's customer scans 2–4× slower than std's: it
paid an iterator and a cache miss per tiny set. Flattening the key, which is
what InnoDB does, fixed both trees. The other indexes keep per-key sets because
they have few keys with many IDs each, the shape B-trees are built for.
`/usr/bin/time -l target/release/examples/index_strategies 1000000 bbtree`
measures peak memory on macOS.

## Verify

```sh
cargo fmt -p conceptual-database -- --check
cargo test --offline -p conceptual-database
cargo clippy --offline -p conceptual-database --all-targets -- -D warnings
```

Tests compare indexed and scan results across filter combinations, boundary
timestamps, ties, and limits; assert work counts; check index movement and batch
rollback; exercise competing writers; swap trees and compare every plan's result
and work counts; and launch the actual HTTP binary on an ephemeral loopback port
for CRUD, validation, transactions, tree swaps, and empty startup.

Start reading [database.rs](src/database.rs) for ownership and writes,
[query.rs](src/query.rs) for access paths, [tree.rs](src/tree.rs) for the two
B-trees, and [http.rs](src/http.rs) for lock scope.
