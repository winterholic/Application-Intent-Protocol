import { aip } from "@aip/define"

export default aip`
  // V1 의미 fixture. C B.1의 네 resource를 그대로 두고, 빠진 기호 선언만 추가했다.
  // 추가 선언(enum, actor, predicate, access, limit, School/Member/ClubMember)은 spike 가정이다.
  
  enum RecruitmentStatus { DRAFT, PUBLISHED, CLOSED }
  enum ApplyStatus { PENDING, APPROVE, REJECT }
  enum ClubRole { ADMIN, MANAGER, MEMBER }
  
  resource School {
    fields { id: Id; name: Text }
  }
  resource Member {
    fields { id: Id; school: School? }
  }
  actor Member
  
  resource ClubMember {
    fields { club: Club; member: Member; role: ClubRole }
    rows read when member = actor
  }
  
  predicate managerOf(m: Member, c: Club) = exists ClubMember where club = c and member = m and role in (ADMIN, MANAGER)
  predicate active(r: Recruitment) = r.status = PUBLISHED and r.periodEnd >= now
  
  access fixedTotalOfVisibleRecruitment = totalOfVisible(Recruitment)
  access clubManagerOnly(a: Member, c: Club.Id) = managerOf(a, c)
  
  limit atMostOnePublished on Recruitment = atMost 1 where status = PUBLISHED
  
  resource Club {
    fields { id: Id; name: Text; logo: Url?; school: School? }
    rows read when school = null or school = actor.school
    expose read { select id, name, logo }
  }
  
  resource RecruitmentBookmark {
    fields { recruitment: Recruitment; member: Member }
    rows read when member = actor
  }
  
  resource Recruitment {
    fields {
      id: Id
      title: Text(1..100)
      periodEnd: Time
      status: RecruitmentStatus
      views: Int
      club: Club
      internalNote: Text?
    }
    rows read when active(this) and
      (club.school = null or club.school = actor.school)
    field internalNote read when managerOf(actor, club)
    expose read {
      select id, title, periodEnd, views, bookmarkCount, internalNote
      filter periodEnd.gte, periodEnd.lte
      sort periodEnd, views, id
      traverse club { select id, name, logo }
      budget { rows 50; depth 2; deadline 2s; cost 1000 }
    }
    aggregate bookmarkCount: Int {
      source RecruitmentBookmark
      sourceAccess fixedTotalOfVisibleRecruitment
      groupKey recruitment
      callerFilter none
      rowOutput none
      release count
    }
    transition close {
      from status = PUBLISHED
      to status = CLOSED
      allow managerOf(actor, club)
    }
    invariant atMostOnePublished per club
    extension read stats {
      input { clubId: Club.Id }
      output { approvedApplicants: Int }
      access Apply.approvedCount
      effect none
      deadline 2s
      implementation "recruitment.stats"
    }
    docs { summary "모집 정보"; visibility internal }
  }
  
  resource Apply {
    fields { id: Id; recruitment: Recruitment; status: ApplyStatus }
    rows read when managerOf(actor, recruitment.club)
    aggregate approvedCount: Int {
      input { clubId: Club.Id }
      sourceAccess clubManagerOnly(actor, input.clubId)
      where recruitment.club.id = input.clubId and status = APPROVE
      callerFilter none
      rowOutput none
      release count
    }
    expose aggregate approvedCount
  }
`
