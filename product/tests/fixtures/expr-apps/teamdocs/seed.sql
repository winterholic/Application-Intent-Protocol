-- 사용자는 조직과 무관한 전역 행이다. 조직 소속과 역할은 Membership으로 따로 둔다(한 사용자가 조직마다 다른 역할).
INSERT INTO ${SCHEMA}."user"(name, email) VALUES
  ('Ann', 'ann@example.test'), ('Ben', 'ben@example.test'), ('Cat', 'cat@example.test'),
  ('Dan', 'dan@example.test'), ('Eve', 'eve@example.test'), ('Fay', 'fay@example.test');
INSERT INTO ${SCHEMA}.org(name) VALUES ('Acme'), ('Beta');
-- Acme: Ann OWNER, Cat ADMIN, Ben·Fay MEMBER. Beta: Dan OWNER, Ann·Eve MEMBER. Ann은 두 조직에 다른 역할로 속한다.
INSERT INTO ${SCHEMA}.membership(org_id, user_id, role) VALUES
  (1, 1, 'OWNER'), (1, 3, 'ADMIN'), (1, 2, 'MEMBER'), (1, 6, 'MEMBER'),
  (2, 4, 'OWNER'), (2, 1, 'MEMBER'), (2, 5, 'MEMBER');
INSERT INTO ${SCHEMA}.doc(org_id, owner_id, title, body, internal_note, state, version) VALUES
  (2, 4, 'beta-draft', 'only dan and beta admins', 'beta-secret-note', 'DRAFT', 0),
  (2, 4, 'beta-public', 'all beta members', 'beta-public-note', 'PUBLISHED', 0);
