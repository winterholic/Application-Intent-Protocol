-- 테넌트(조직)와 사용자 프로비저닝은 앱 정의 밖(운영자 SQL 또는 IdP 연동)에서 한다. id는 identity가 1부터 부여한다.
INSERT INTO ${SCHEMA}.org(name) VALUES ('Acme'), ('Globex');
INSERT INTO ${SCHEMA}."user"(org_id, name, email, role) VALUES
  (1, 'Alice', 'alice@acme.test', 'OWNER'),
  (1, 'Bob', 'bob@acme.test', 'MEMBER'),
  (1, 'Vera', 'vera@acme.test', 'VIEWER'),
  (2, 'Carol', 'carol@globex.test', 'ADMIN'),
  (2, 'Dave', 'dave@globex.test', 'MEMBER');
-- 다른 테넌트(Globex)의 프로젝트·할 일. Acme 사용자는 보지도 바꾸지도 못해야 한다.
INSERT INTO ${SCHEMA}.project(org_id, name, status, owner_id, version) VALUES (2, 'Globex roadmap', 'ACTIVE', 4, 0);
INSERT INTO ${SCHEMA}.task(org_id, project_id, title, description, assignee_id, creator_id, status, priority, version)
  VALUES (2, 1, 'globex-secret-task', 'confidential', 5, 4, 'TODO', 3, 0);
