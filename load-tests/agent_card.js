// A2A Agent Card discovery throughput test.
// Run: BASE_URL=http://localhost:3000 k6 run load-tests/agent_card.js

import http from 'k6/http';
import { check, sleep } from 'k6';

export const options = {
  vus: 20,
  duration: '60s',
  thresholds: {
    http_req_duration: ['p(95)<200'],
    http_req_failed: ['rate<0.01'],
  },
};

export default function () {
  const res = http.get(`${__ENV.BASE_URL}/.well-known/agent.json`);
  check(res, {
    'status 200': (r) => r.status === 200,
    'has name': (r) => JSON.parse(r.body).name !== undefined,
    'has capabilities': (r) => JSON.parse(r.body).capabilities !== undefined,
  });
  sleep(0.1);
}
