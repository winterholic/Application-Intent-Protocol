//! Structural encryption constraints. Runtime activation remains independently blocked in sema::type_of.
use crate::{ast::*, diag::Diag};
use std::collections::{BTreeMap, BTreeSet};

pub(crate) fn validate(spec: &Spec) -> Vec<Diag> {
    let mut errors = Vec::new();
    let mut scopes: BTreeMap<&str, BTreeSet<&str>> = BTreeMap::new();
    let actor = spec.actor.as_ref().map(|(actor, _)| actor.as_str());
    for resource in &spec.resources {
        for field in &resource.fields {
            let Some(enc) = &field.ty.encrypted else { continue };
            if field.name == "id" {
                errors.push(Diag::new("DEF.ENCRYPTED_ID_FORBIDDEN", "암호화된 기본키는 지원하지 않음", field.span));
            }
            if !resource.fields.iter().any(|field| field.name == "id") {
                errors.push(Diag::new("DEF.ENCRYPTED_REQUIRES_ID", "암호화 AAD에는 명시적 행 id가 필요", enc.span));
            }
            let bound = match enc.scope.as_str() {
                "service" => None,
                "owner" if actor == Some(resource.name.as_str()) => None,
                "owner" => {
                    let candidates: Vec<_> = resource
                        .fields
                        .iter()
                        .filter(|field| {
                            !field.ty.nullable && !field.ty.id_of && field.ty.encrypted.is_none() && actor == Some(field.ty.name.as_str())
                        })
                        .collect();
                    if candidates.len() != 1 {
                        errors.push(Diag::new(
                            "DEF.ENCRYPTED_AMBIGUOUS_OWNER",
                            "owner는 actor 행 또는 단 하나의 필수 actor 참조로 결속되어야 함",
                            enc.span,
                        ));
                    }
                    (candidates.len() == 1).then(|| candidates[0])
                }
                name => {
                    let bound = resource.fields.iter().find(|candidate| {
                        candidate.name == name
                            && !candidate.ty.nullable
                            && !candidate.ty.id_of
                            && candidate.ty.encrypted.is_none()
                            && spec.resources.iter().any(|target| target.name == candidate.ty.name)
                    });
                    if bound.is_none() {
                        errors.push(Diag::new(
                            "DEF.ENCRYPTED_BAD_SCOPE",
                            "scope는 service 또는 같은 행의 필수 비암호화 resource 참조여야 함",
                            enc.span,
                        ));
                    }
                    bound
                }
            };
            if let Some(bound) = bound {
                scopes.entry(&resource.name).or_default().insert(&bound.name);
            }
            if !enc.lookup_eq && resource.uniques.iter().any(|(columns, _)| columns.iter().any(|(name, _)| name == &field.name)) {
                errors.push(Diag::new("DEF.ENCRYPTED_UNIQUE_REQUIRES_LOOKUP", "암호화 unique에는 lookup: eq가 필요", enc.span));
            }
        }
        if let Some(read) = &resource.expose_read {
            for (field, op, span) in &read.filter {
                if let Some(enc) = encrypted(resource, field) {
                    if !(enc.lookup_eq && matches!(op.as_str(), "eq" | "in" | "isNull")) {
                        errors.push(Diag::new("DEF.ENCRYPTED_FILTER_FORBIDDEN", "암호화 필터는 lookup: eq의 동치 연산만 허용", *span));
                    }
                }
            }
            for (field, span) in &read.sort {
                if encrypted(resource, field).is_some() {
                    errors.push(Diag::new("DEF.ENCRYPTED_SORT_FORBIDDEN", "암호화 필드로 정렬할 수 없음", *span));
                }
            }
            for many in &read.traverse_many {
                if let Some((field, _, span)) = &many.sort {
                    if spec.resources.iter().find(|target| target.name == many.child).is_some_and(|target| encrypted(target, field).is_some()) {
                        errors.push(Diag::new("DEF.ENCRYPTED_SORT_FORBIDDEN", "암호화된 자식 필드로 정렬할 수 없음", *span));
                    }
                }
            }
        }
        for aggregate in &resource.aggregates {
            if aggregate.ty.encrypted.is_some() {
                errors.push(Diag::new("DEF.ENCRYPTED_LOCATION_FORBIDDEN", "aggregate 타입에 encrypted를 선언할 수 없음", aggregate.span));
            }
            let source = aggregate.source.as_deref().unwrap_or(&resource.name);
            let Some(source) = spec.resources.iter().find(|target| target.name == source) else { continue };
            let release_field =
                aggregate.release.as_deref().and_then(|release| release.split_once('(')).and_then(|(_, field)| field.strip_suffix(')'));
            if release_field.is_some_and(|field| encrypted(source, field).is_some())
                || aggregate.group_key.as_deref().is_some_and(|field| encrypted(source, field).is_some())
            {
                errors.push(Diag::new("DEF.ENCRYPTED_AGGREGATE_FORBIDDEN", "암호화 값은 집계·group key로 사용할 수 없음", aggregate.span));
            }
        }
    }
    for resource in &spec.resources {
        for transition in &resource.transitions {
            for (field, _) in &transition.to {
                if encrypted(resource, field).is_some() {
                    errors.push(Diag::new(
                        "DEF.ENCRYPTED_TRANSITION_VALUE_FORBIDDEN",
                        "암호화 필드에 정의 상수를 대입할 수 없음. 호출자의 patch 값을 쓴다",
                        transition.span,
                    ));
                }
            }
            for (field, span) in &transition.expect {
                if encrypted(resource, field).is_some() {
                    errors.push(Diag::new("DEF.ENCRYPTED_TRANSITION_VALUE_FORBIDDEN", "암호화 필드에는 expect를 쓸 수 없음", *span));
                }
            }
            for field in transition.to.iter().map(|(name, _)| name).chain(transition.patch.iter().map(|(name, _)| name)) {
                if scopes.get(resource.name.as_str()).is_some_and(|scope| scope.contains(field.as_str())) {
                    errors.push(Diag::new("DEF.ENCRYPTED_MUTABLE_SCOPE", "암호화 scope 참조는 전이로 바꿀 수 없음", transition.span));
                }
            }
            for effect in &transition.effects {
                let (target, values, update, span) = match effect {
                    Effect::Create { resource, values, span } => (resource, values, false, span),
                    Effect::Update { resource, values, span, .. } => (resource, values, true, span),
                    _ => continue,
                };
                let Some(target_resource) = spec.resources.iter().find(|resource| &resource.name == target) else { continue };
                for (field, _) in values {
                    if encrypted(target_resource, field).is_some() {
                        errors.push(Diag::new("DEF.ENCRYPTED_TRANSITION_VALUE_FORBIDDEN", "전이 효과에 암호화 값을 대입할 수 없음", *span));
                    }
                    if update && scopes.get(target.as_str()).is_some_and(|scope| scope.contains(field.as_str())) {
                        errors.push(Diag::new("DEF.ENCRYPTED_MUTABLE_SCOPE", "전이 효과로 암호화 scope를 바꿀 수 없음", *span));
                    }
                }
            }
        }
    }
    errors
}

fn encrypted<'a>(resource: &'a Resource, field: &str) -> Option<&'a Encryption> {
    resource.fields.iter().find(|candidate| candidate.name == field).and_then(|field| field.ty.encrypted.as_ref())
}
