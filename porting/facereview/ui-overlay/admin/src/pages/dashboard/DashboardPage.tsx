import { useState } from 'react';
import type { ReactElement } from 'react';
import { Link } from 'react-router-dom';
import { useQuery } from '@tanstack/react-query';
import { isAxiosError } from 'axios';
import { ResponsiveLine } from '@nivo/line';
import { ResponsiveBar } from '@nivo/bar';
import { ResponsivePie } from '@nivo/pie';
import { getBusinessStats, getSignupTrend, getSystemStatus } from 'api/admin';
import { hostMetricsLabel } from '../../dashboardStatus';
import type { SignupTrendPeriod } from 'types';
import StatusChip from 'components/StatusChip/StatusChip';
import { nivoDarkTheme, nivoDarkThemeCompactAxis, EMOTION_COLOR, EMOTION_LABEL } from './nivoTheme';

import './dashboardPage.scss';

const CONNECTION_LABEL: Record<string, string> = {
  mysql: 'MySQL',
  redis: 'Redis',
  mongodb: 'MongoDB',
};

const PERIOD_OPTIONS: { value: SignupTrendPeriod; label: string }[] = [
  { value: '7d', label: '최근 7일' },
  { value: '30d', label: '최근 30일' },
  { value: '3m', label: '최근 3달' },
  { value: '1y', label: '최근 1년' },
  { value: '3y', label: '최근 3년' },
];

const formatTrendLabel = (date: string, granularity: 'day' | 'week' | 'month'): string => {
  const [year, month, day] = date.split('-');
  if (granularity === 'month') return `${year}-${month}`;
  return `${month}-${day}`;
};

const formatTooltipDate = (date: string, granularity: 'day' | 'week' | 'month'): string => {
  const [year, month, day] = date.split('-');
  if (granularity === 'month') return `${year}년 ${Number(month)}월`;
  if (granularity === 'week') return `${year}년 ${Number(month)}월 ${Number(day)}일 주`;
  return `${year}년 ${Number(month)}월 ${Number(day)}일`;
};

const pickThinnedTickValues = (values: string[], maxTicks: number): string[] | undefined => {
  if (values.length <= maxTicks) return undefined;
  const step = Math.ceil(values.length / maxTicks);
  const picked = values.filter((_, index) => index % step === 0);
  const last = values.at(-1);
  if (last && picked.at(-1) !== last) picked.push(last);
  return picked;
};

const getErrorMessage = (error: unknown): string => {
  if (isAxiosError(error)) {
    const status = error.response?.status;
    const serverMessage = (error.response?.data as { message?: string } | undefined)?.message;
    return `${status ?? '네트워크 오류'}${serverMessage ? ` - ${serverMessage}` : ` - ${error.message}`}`;
  }
  return error instanceof Error ? error.message : '알 수 없는 오류';
};

const DashboardPage = (): ReactElement => {
  const {
    data: stats,
    isLoading: statsLoading,
    isError: statsIsError,
    error: statsError,
    refetch: refetchStats,
  } = useQuery({
    queryKey: ['admin', 'business-stats'],
    queryFn: async () => (await getBusinessStats()).data,
    refetchInterval: 60000,
  });

  const { data: system } = useQuery({
    queryKey: ['admin', 'system-status'],
    queryFn: async () => (await getSystemStatus()).data,
    refetchInterval: 30000,
  });

  const [trendPeriod, setTrendPeriod] = useState<SignupTrendPeriod>('7d');
  const { data: trend, isLoading: trendLoading } = useQuery({
    queryKey: ['admin', 'signup-trend', trendPeriod],
    queryFn: async () => (await getSignupTrend(trendPeriod)).data,
  });

  const todaySignups = stats?.signup_trend.at(-1)?.count ?? 0;
  const weekSignups = stats?.signup_trend.reduce((sum, point) => sum + point.count, 0) ?? 0;
  const hostMetrics = system ? hostMetricsLabel(system.server) : null;

  const signupLineData = trend
    ? [
        {
          id: '신규가입',
          data: trend.points.map((point) => ({
            x: formatTrendLabel(point.date, trend.granularity),
            y: point.count,
            rawDate: point.date,
          })),
        },
      ]
    : [];

  const signupTickValues = trend
    ? pickThinnedTickValues(
        trend.points.map((point) => formatTrendLabel(point.date, trend.granularity)),
        8,
      )
    : undefined;

  const categoryBarData = stats
    ? stats.content_health.category_top5.map((c) => ({
        category: c.category,
        view_count: c.view_count,
      }))
    : [];

  const emotionPieData = stats
    ? (Object.entries(stats.content_health.emotion_distribution) as [string, number][])
        .sort((a, b) => b[1] - a[1])
        .map(([emotion, ratio]) => ({
          id: EMOTION_LABEL[emotion] ?? emotion,
          label: EMOTION_LABEL[emotion] ?? emotion,
          value: Math.round(ratio * 1000) / 10,
          color: EMOTION_COLOR[emotion] ?? '#5D5D6D',
        }))
    : [];

  const dominantCounts = stats?.content_health.dominant_emotion_video_counts ?? [];

  return (
    <div className="dashboard-page">
      <h2 className="font-title-large">대시보드</h2>

      <div className="overview-strip">
        <div className="overview-strip__row overview-strip__row--connections">
          {system ? (
            Object.entries(system.connections).map(([key, status]) => (
              <div key={key} className="overview-strip__connection">
                <span className="font-label-small">{CONNECTION_LABEL[key] ?? key}</span>
                <StatusChip
                  label={status === 'ok' ? '정상' : '오류'}
                  tone={status === 'ok' ? 'accepted' : 'danger'}
                />
              </div>
            ))
          ) : (
            <span className="font-label-small overview-strip__muted">시스템 상태 확인 중...</span>
          )}
          {system && (
            <span className="overview-strip__sysmetrics font-label-small">
              {hostMetrics} · API {system.api.total_requests_1h.toLocaleString()}
              건/1h · 평균 {system.api.avg_response_time_ms.toFixed(0)}ms · 에러율{' '}
              {system.api.error_rate_1h.toFixed(2)}%
            </span>
          )}
        </div>

        <div className="overview-strip__row overview-strip__row--kpi">
          {statsIsError ? (
            <span className="font-label-small overview-strip__error">
              비즈니스 지표 로드 실패 ({getErrorMessage(statsError)}){' '}
              <button type="button" className="overview-strip__retry" onClick={() => refetchStats()}>
                다시 시도
              </button>
            </span>
          ) : statsLoading || !stats ? (
            <span className="font-label-small overview-strip__muted">불러오는 중...</span>
          ) : (
            <>
              <div className="kpi-stat">
                <span className="kpi-stat__label font-label-small">주간 활성 시청자</span>
                <span className="kpi-stat__value font-title-mini">
                  {stats.weekly_active_users.toLocaleString()}
                </span>
              </div>
              <div className="kpi-stat">
                <span className="kpi-stat__label font-label-small">평균 완주율</span>
                <span className="kpi-stat__value font-title-mini">
                  {(stats.content_health.avg_completion_rate * 100).toFixed(1)}%
                </span>
              </div>
              <div className="kpi-stat">
                <span className="kpi-stat__label font-label-small">
                  오늘 신규가입 <em>(7일 합계 {weekSignups})</em>
                </span>
                <span className="kpi-stat__value font-title-mini">{todaySignups}</span>
              </div>
              <Link to="/video-requests?status=PENDING" className="kpi-stat kpi-stat--link">
                <span className="kpi-stat__label font-label-small">
                  영상요청 대기{' '}
                  <em>
                    (평균{' '}
                    {stats.video_request_pipeline.avg_processing_minutes === null
                      ? '최근 처리 이력 없음'
                      : `${stats.video_request_pipeline.avg_processing_minutes.toFixed(0)}분`}
                    )
                  </em>
                </span>
                <span className="kpi-stat__value font-title-mini">
                  {stats.video_request_pipeline.pending_count}
                </span>
              </Link>
            </>
          )}
        </div>
      </div>

      {!statsLoading && stats && (
        <>
          <div className="chart-row-two">
            <section className="chart-panel">
              <div className="chart-panel__header">
                <h3 className="font-title-mini">신규가입 추이</h3>
                <div className="chart-panel__period-select">
                  {PERIOD_OPTIONS.map((option) => (
                    <button
                      key={option.value}
                      type="button"
                      className={`chart-panel__period-button font-label-small${
                        trendPeriod === option.value ? ' active' : ''
                      }`}
                      onClick={() => setTrendPeriod(option.value)}>
                      {option.label}
                    </button>
                  ))}
                </div>
              </div>
              <div className="chart-panel__body">
                {trendLoading || !trend ? (
                  <p className="chart-panel__loading font-body-medium">불러오는 중...</p>
                ) : (
                  <ResponsiveLine
                    data={signupLineData}
                    theme={nivoDarkTheme}
                    colors={['#76FFCE']}
                    margin={{ top: 16, right: 20, bottom: 32, left: 32 }}
                    xScale={{ type: 'point' }}
                    yScale={{ type: 'linear', min: 0, max: 'auto' }}
                    curve="monotoneX"
                    axisBottom={{ tickSize: 0, tickPadding: 8, tickRotation: 0, tickValues: signupTickValues }}
                    axisLeft={{ tickSize: 0, tickPadding: 8, tickValues: 4 }}
                    enableGridX={false}
                    pointSize={signupLineData[0]?.data.length > 40 ? 0 : 8}
                    pointColor={{ theme: 'background' }}
                    pointBorderWidth={2}
                    pointBorderColor={{ from: 'serieColor' }}
                    useMesh
                    tooltip={({ point }) => (
                      <div className="chart-panel__tooltip">
                        <strong>
                          {formatTooltipDate(point.data.rawDate as string, trend?.granularity ?? 'day')}
                        </strong>
                        <br />
                        신규가입 {point.data.y as number}명
                      </div>
                    )}
                  />
                )}
              </div>
            </section>

            <section className="chart-panel">
              <h3 className="font-title-mini">시청 감정 분포</h3>
              <div className="chart-panel__pie-row">
                <div className="chart-panel__body chart-panel__body--pie">
                  <ResponsivePie
                    data={emotionPieData}
                    theme={nivoDarkTheme}
                    colors={{ datum: 'data.color' }}
                    margin={{ top: 8, right: 8, bottom: 8, left: 8 }}
                    innerRadius={0.65}
                    padAngle={1.5}
                    cornerRadius={3}
                    borderWidth={1}
                    borderColor={{ from: 'color', modifiers: [['darker', 0.2]] }}
                    enableArcLinkLabels={false}
                    arcLabel={(d) => `${d.value}%`}
                    arcLabelsTextColor="#15151D"
                    tooltip={({ datum }) => (
                      <div className="chart-panel__tooltip">
                        {datum.label}: {datum.value}%
                      </div>
                    )}
                  />
                </div>
                <ul className="chart-panel__legend chart-panel__legend--side">
                  {emotionPieData.map((d) => (
                    <li key={d.id}>
                      <span className="chart-panel__legend-dot" style={{ backgroundColor: d.color }} />
                      <span className="font-label-small">{d.label}</span>
                      <span className="font-label-small chart-panel__legend-value">{d.value}%</span>
                    </li>
                  ))}
                </ul>
              </div>
              {dominantCounts.length > 0 && (
                <p className="chart-panel__hint font-label-small">
                  대표 감정 기준 영상 수:{' '}
                  {dominantCounts
                    .map((d) => `${EMOTION_LABEL[d.emotion] ?? d.emotion} ${d.video_count}개`)
                    .join(' · ')}
                </p>
              )}
            </section>
          </div>

          <section className="chart-panel chart-panel--wide">
            <h3 className="font-title-mini">카테고리별 조회수</h3>
            <div className="chart-panel__body chart-panel__body--wide">
              <ResponsiveBar
                data={categoryBarData}
                theme={nivoDarkThemeCompactAxis}
                keys={['view_count']}
                indexBy="category"
                colors={['#F47263']}
                margin={{ top: 8, right: 16, bottom: 36, left: 56 }}
                padding={0.35}
                borderRadius={4}
                axisBottom={{ tickSize: 0, tickPadding: 8, tickRotation: 0, truncateTickAt: 0 }}
                axisLeft={{ tickSize: 0, tickPadding: 8 }}
                enableGridY
                enableLabel={false}
                tooltip={({ indexValue, value }) => (
                  <div className="chart-panel__tooltip">
                    {indexValue}: {value.toLocaleString()}
                  </div>
                )}
              />
            </div>
          </section>
        </>
      )}
    </div>
  );
};

export default DashboardPage;
