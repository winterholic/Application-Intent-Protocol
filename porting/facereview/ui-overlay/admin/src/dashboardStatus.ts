export interface HostMetrics {
  cpu_usage: number | null;
  memory_usage: number | null;
  disk_usage: number | null;
}

/** Host figures are intentionally unavailable on non-Linux service hosts; never turn null into a measured zero. */
export function hostMetricsLabel(server: HostMetrics): string {
  if (
    server.cpu_usage === null || server.memory_usage === null || server.disk_usage === null ||
    !Number.isFinite(server.cpu_usage) || !Number.isFinite(server.memory_usage) || !Number.isFinite(server.disk_usage)
  ) {
    return '호스트 지표를 사용할 수 없습니다.';
  }
  return `CPU ${server.cpu_usage.toFixed(0)}% · 메모리 ${server.memory_usage.toFixed(0)}% · 디스크 ${server.disk_usage.toFixed(0)}%`;
}
