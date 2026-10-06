import { define } from "@aip/define"

// H 대조군: C B.3의 정형 데이터 선언에 A fixture와 같은 추가 기호 선언을 넣었다.
export const spec = define({
  enums: {
    RecruitmentStatus: ["DRAFT", "PUBLISHED", "CLOSED"],
    ApplyStatus: ["PENDING", "APPROVE", "REJECT"],
    ClubRole: ["ADMIN", "MANAGER", "MEMBER"],
  },
  actor: "Member",
  predicates: {
    managerOf: {
      params: { m: "Member", c: "Club" },
      body: "exists ClubMember where club = c and member = m and role in (ADMIN, MANAGER)",
    },
    active: {
      params: { r: "Recruitment" },
      body: "r.status = PUBLISHED and r.periodEnd >= now",
    },
  },
  access: {
    fixedTotalOfVisibleRecruitment: { totalOfVisible: "Recruitment" },
    clubManagerOnly: { params: { a: "Member", c: "Club.Id" }, guard: "managerOf(a, c)" },
  },
  limits: {
    atMostOnePublished: { on: "Recruitment", atMost: 1, where: "status = PUBLISHED" },
  },
  resources: {
    School: { fields: { id: "Id", name: "Text" } },
    Member: { fields: { id: "Id", school: "School?" } },
    ClubMember: {
      fields: { club: "Club", member: "Member", role: "ClubRole" },
      rows: "member = actor",
    },
    Club: {
      fields: { id: "Id", name: "Text", logo: "Url?", school: "School?" },
      rows: "school = null or school = actor.school",
      read: { select: ["id", "name", "logo"] },
    },
    RecruitmentBookmark: {
      fields: { recruitment: "Recruitment", member: "Member" },
      rows: "member = actor",
    },
    Recruitment: {
      fields: {
        id: "Id", title: "Text(1..100)", periodEnd: "Time",
        status: "RecruitmentStatus", views: "Int", club: "Club",
        internalNote: "Text?",
      },
      rows: "active(this) and (club.school = null or club.school = actor.school)",
      fieldRead: { internalNote: "managerOf(actor, club)" },
      read: {
        select: ["id", "title", "periodEnd", "views", "bookmarkCount", "internalNote"],
        filter: ["periodEnd.gte", "periodEnd.lte"],
        sort: ["periodEnd", "views", "id"],
        traverse: { club: { select: ["id", "name", "logo"] } },
        budget: { rows: 50, depth: 2, deadline: "2s", cost: 1000 },
      },
      aggregates: {
        bookmarkCount: {
          type: "Int", source: "RecruitmentBookmark",
          sourceAccess: "fixedTotalOfVisibleRecruitment", groupKey: "recruitment",
          callerFilter: "none", rowOutput: "none", release: "count",
        },
      },
      transitions: {
        close: { from: "status = PUBLISHED", to: "status = CLOSED", allow: "managerOf(actor, club)" },
      },
      invariants: ["atMostOnePublished per club"],
      extensions: {
        stats: {
          kind: "read", input: { clubId: "Club.Id" }, output: { approvedApplicants: "Int" },
          access: ["Apply.approvedCount"], effect: "none",
          deadline: "2s", implementation: "recruitment.stats",
        },
      },
      docs: { summary: "모집 정보", visibility: "internal" },
    },
    Apply: {
      fields: { id: "Id", recruitment: "Recruitment", status: "ApplyStatus" },
      rows: "managerOf(actor, recruitment.club)",
      aggregates: {
        approvedCount: {
          type: "Int", input: { clubId: "Club.Id" },
          sourceAccess: "clubManagerOnly(actor, input.clubId)",
          where: "recruitment.club.id = input.clubId and status = APPROVE",
          callerFilter: "none", rowOutput: "none", release: "count",
        },
      },
      exposeAggregate: ["approvedCount"],
    },
  },
})
