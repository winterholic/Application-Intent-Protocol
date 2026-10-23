import assert from 'node:assert/strict';
import test from 'node:test';
import { hostMetricsLabel } from '../admin/src/dashboardStatus.ts';

test('null host metrics produce an unavailable state instead of a fabricated percentage', () => {
  assert.equal(hostMetricsLabel({ cpu_usage: null, memory_usage: null, disk_usage: null }), '호스트 지표를 사용할 수 없습니다.');
});

test('available host metrics retain their measured values', () => {
  assert.equal(hostMetricsLabel({ cpu_usage: 53, memory_usage: 21, disk_usage: 59 }), 'CPU 53% · 메모리 21% · 디스크 59%');
});

test('partial host metrics do not display a misleading partial percentage row', () => {
  assert.equal(hostMetricsLabel({ cpu_usage: 53, memory_usage: null, disk_usage: 59 }), '호스트 지표를 사용할 수 없습니다.');
});
