// Baseline health endpoint load test.
// Run: BASE_URL=http://localhost:3000 k6 run load-tests/health.js

import http from 'k6/http';
import { check } from 'k6';

export const options = {
  vus: 10,
  duration: '30s',
  thresholds: {
    http_req_duration: ['p(99)<100'],
    http_req_failed: ['rate<0.01'],
  },
};

export default function () {
  const res = http.get(`${__ENV.BASE_URL}/health`);
  check(res, {
    'status 200': (r) => r.status === 200,
  });
}
