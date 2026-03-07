// A2A tasks/send sustained load test with ramp-up/ramp-down.
// Run: BASE_URL=http://localhost:3000 BEARER_TOKEN=your-token k6 run load-tests/a2a_task_send.js

import http from 'k6/http';
import { check } from 'k6';

const BASE_URL = __ENV.BASE_URL || 'http://localhost:3000';
const TOKEN = __ENV.BEARER_TOKEN;

export const options = {
  stages: [
    { duration: '10s', target: 5 },
    { duration: '30s', target: 5 },
    { duration: '10s', target: 0 },
  ],
  thresholds: {
    http_req_duration: ['p(95)<500'],
    http_req_failed: ['rate<0.01'],
  },
};

export default function () {
  const payload = JSON.stringify({
    jsonrpc: '2.0',
    method: 'tasks/send',
    params: {
      message: {
        role: 'user',
        parts: [{ type: 'text', text: `k6 load test ${Date.now()}` }],
      },
    },
    id: 1,
  });

  const params = {
    headers: {
      'Content-Type': 'application/json',
      Authorization: `Bearer ${TOKEN}`,
    },
  };

  const res = http.post(`${BASE_URL}/a2a`, payload, params);
  check(res, {
    'status 200': (r) => r.status === 200,
    'jsonrpc 2.0': (r) => JSON.parse(r.body).jsonrpc === '2.0',
  });
}
