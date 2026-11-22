-- 역할은 시드에서 정한다: 1 Olga·2 Pavel 고객, 3 Sam 직원, 4 Gate 결제 test double.
INSERT INTO ${SCHEMA}.member(name, role) VALUES ('Olga', 'CUSTOMER'), ('Pavel', 'CUSTOMER'), ('Sam', 'STAFF'), ('Gate', 'GATEWAY');
INSERT INTO ${SCHEMA}.product(sku, stock, price, cost) VALUES
  ('A', 10, 200, 120), ('B', 5, 50, 30), ('C', 4, 10, 6), ('D', 4, 10, 6), ('E', 6, 100, 60), ('F', 6, 100, 60);
