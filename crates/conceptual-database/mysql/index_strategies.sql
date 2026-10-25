-- The index_strategies example in real InnoDB: same distributions, same five
-- strategies, forced with optimizer hints. See the README for the docker command.

DROP TABLE IF EXISTS orders;
CREATE TABLE orders (
  order_id    BIGINT UNSIGNED NOT NULL PRIMARY KEY,
  customer_id BIGINT UNSIGNED NOT NULL,
  state       ENUM('placed', 'shipped', 'cancelled') NOT NULL,
  product_id  BIGINT UNSIGNED NOT NULL,
  placed_at   BIGINT UNSIGNED NOT NULL,
  INDEX idx_state (state),
  INDEX idx_product (product_id),
  INDEX idx_customer_date (customer_id, placed_at),
  INDEX idx_product_state (product_id, state)
);

-- 1M orders: 10% hot product 0; 90% shipped, 8% placed, 2% cancelled.
SET SESSION cte_max_recursion_depth = 1000000;
INSERT INTO orders
WITH RECURSIVE seq (n) AS (SELECT 0 UNION ALL SELECT n + 1 FROM seq WHERE n < 999999)
SELECT
  n,
  CRC32(CONCAT('c', n)) % 100000,
  CASE
    WHEN CRC32(CONCAT('s', n)) % 100 < 2 THEN 'cancelled'
    WHEN CRC32(CONCAT('s', n)) % 100 < 10 THEN 'placed'
    ELSE 'shipped'
  END,
  IF(CRC32(CONCAT('p', n)) % 10 = 0, 0, 1 + CRC32(CONCAT('q', n)) % 999),
  n
FROM seq;
ANALYZE TABLE orders;

SELECT state, COUNT(*) FROM orders GROUP BY state;
SELECT COUNT(*) AS product_7 FROM orders WHERE product_id = 7;
SELECT COUNT(*) AS product_0 FROM orders WHERE product_id = 0;

-- Typical product, rare state. Each strategy forced in turn, then the optimizer's
-- own choice with and without the composite index available.
EXPLAIN ANALYZE SELECT /*+ NO_INDEX(orders) */ * FROM orders WHERE product_id = 7 AND state = 'cancelled';
EXPLAIN ANALYZE SELECT * FROM orders FORCE INDEX (idx_state) WHERE product_id = 7 AND state = 'cancelled';
EXPLAIN ANALYZE SELECT * FROM orders FORCE INDEX (idx_product) WHERE product_id = 7 AND state = 'cancelled';
EXPLAIN ANALYZE SELECT /*+ INDEX_MERGE(orders idx_product, idx_state) */ * FROM orders WHERE product_id = 7 AND state = 'cancelled';
EXPLAIN ANALYZE SELECT * FROM orders FORCE INDEX (idx_product_state) WHERE product_id = 7 AND state = 'cancelled';
EXPLAIN SELECT * FROM orders WHERE product_id = 7 AND state = 'cancelled';
EXPLAIN SELECT /*+ NO_INDEX(orders idx_product_state) */ * FROM orders WHERE product_id = 7 AND state = 'cancelled';

-- Hot product, rare state: the case where intersection fetches far fewer rows.
EXPLAIN ANALYZE SELECT * FROM orders FORCE INDEX (idx_state) WHERE product_id = 0 AND state = 'cancelled';
EXPLAIN ANALYZE SELECT * FROM orders FORCE INDEX (idx_product) WHERE product_id = 0 AND state = 'cancelled';
EXPLAIN ANALYZE SELECT /*+ INDEX_MERGE(orders idx_product, idx_state) */ * FROM orders WHERE product_id = 0 AND state = 'cancelled';
EXPLAIN ANALYZE SELECT * FROM orders FORCE INDEX (idx_product_state) WHERE product_id = 0 AND state = 'cancelled';
EXPLAIN SELECT /*+ NO_INDEX(orders idx_product_state) */ * FROM orders WHERE product_id = 0 AND state = 'cancelled';

-- Intersection needs each index's matching entries in primary-key order. A range
-- on placed_at breaks that for idx_customer_date, so this cannot intersect.
EXPLAIN SELECT /*+ INDEX_MERGE(orders idx_customer_date, idx_state) */ * FROM orders
WHERE customer_id = 42 AND placed_at > 500000 AND state = 'cancelled';
