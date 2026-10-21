// Invented data shaped like the CP API (agentiflow detail + children + message log; a workflow run graph + its message log).
window.FIXTURE = {
 "agentiflow": {
  "record": {
   "id": "af_01M5MOCKFLOW",
   "name": "acme-shop-auth",
   "codename": "cobalt-harbor",
   "status": "running",
   "rounds": 4,
   "goals_met": 1,
   "goals_total": 2,
   "spent_usd": 41.37,
   "spent_tokens": 6060000,
   "started_at": "2026-10-20T14:00:00Z",
   "ended_at": null
  },
  "def": {
   "description": "Authorized test of the acme-shop staging auth surface: find and verify real authentication and authorization flaws.",
   "lead": "audit-lead",
   "goals": [
    {
     "id": "verified",
     "objective": "At least 2 verified auth findings",
     "met": true,
     "current": 2,
     "target": 2
    },
    {
     "id": "report",
     "objective": "An engagement report",
     "met": false,
     "current": 0,
     "target": 1
    }
   ],
   "budget": {
    "usd": 150,
    "rounds": 10
   }
  },
  "lead": {
   "codename": "cobalt-harbor/heron",
   "agent": "audit-lead",
   "address": "lead"
  },
  "rounds": [
   {
    "round": 0,
    "start": 1,
    "end": 23,
    "goals_met": 0,
    "summary": "Map the surface before testing anything.",
    "dispatched": [
     "lynx#1",
     "lynx#2",
     "lynx#3"
    ]
   },
   {
    "round": 1,
    "start": 24,
    "end": 84,
    "goals_met": 0,
    "summary": "Generated auth-sweep to probe every auth endpoint, with a review panel.",
    "dispatched": [
     "gannet#1"
    ],
    "generated": [
     "auth-sweep"
    ]
   },
   {
    "round": 2,
    "start": 85,
    "end": 111,
    "goals_met": 1,
    "summary": "Verify the two highest findings; retry the verifier that hit a 529.",
    "dispatched": [
     "kite#1",
     "kite#2",
     "kite#3"
    ]
   },
   {
    "round": 3,
    "start": 112,
    "end": null,
    "goals_met": 1,
    "summary": "Write the engagement report.",
    "dispatched": [
     "owl#1"
    ]
   }
  ],
  "children": [
   {
    "handle": "lynx#1",
    "kind": "agent",
    "name": "surface-mapper",
    "run_id": "run_lynx#1",
    "parent": "lead",
    "depth": 1,
    "codename": "cobalt-harbor/heron>lynx#1",
    "address": "lynx#1",
    "status": "done",
    "success": true,
    "error": null,
    "round": 0,
    "started_at": "2026-10-20T14:02:00Z",
    "finished_at": "2026-10-20T14:19:00Z",
    "tokens": {
     "input": 410000,
     "output": 9800
    },
    "output": "Mapped 14 routes, 3 auth flows, 2 admin panels."
   },
   {
    "handle": "lynx#2",
    "kind": "agent",
    "name": "dependency-auditor",
    "run_id": "run_lynx#2",
    "parent": "lead",
    "depth": 1,
    "codename": "cobalt-harbor/heron>lynx#2",
    "address": "lynx#2",
    "status": "done",
    "success": true,
    "error": null,
    "round": 0,
    "started_at": "2026-10-20T14:02:00Z",
    "finished_at": "2026-10-20T14:15:00Z",
    "tokens": {
     "input": 260000,
     "output": 6100
    },
    "output": "9 outdated packages, 1 with a known advisory (session lib)."
   },
   {
    "handle": "lynx#3",
    "kind": "agent",
    "name": "config-reviewer",
    "run_id": "run_lynx#3",
    "parent": "lead",
    "depth": 1,
    "codename": "cobalt-harbor/heron>lynx#3",
    "address": "lynx#3",
    "status": "done",
    "success": true,
    "error": null,
    "round": 0,
    "started_at": "2026-10-20T14:03:00Z",
    "finished_at": "2026-10-20T14:22:00Z",
    "tokens": {
     "input": 330000,
     "output": 7400
    },
    "output": "Debug toolbar enabled in staging config; CORS allows *."
   },
   {
    "handle": "lynx#1>wren#1",
    "kind": "subagent",
    "name": "route-crawler",
    "run_id": "run_lynx#1>wren#1",
    "parent": "lynx#1",
    "depth": 2,
    "codename": "cobalt-harbor/heron>lynx#1>wren#1",
    "address": "lynx#1>wren#1",
    "status": "done",
    "success": true,
    "error": null,
    "round": 0,
    "started_at": "2026-10-20T14:05:00Z",
    "finished_at": "2026-10-20T14:11:00Z",
    "tokens": {
     "input": 90000,
     "output": 2100
    },
    "output": "Crawled /api, /admin, /auth."
   },
   {
    "handle": "lynx#1>wren#2",
    "kind": "subagent",
    "name": "js-bundle-reader",
    "run_id": "run_lynx#1>wren#2",
    "parent": "lynx#1",
    "depth": 2,
    "codename": "cobalt-harbor/heron>lynx#1>wren#2",
    "address": "lynx#1>wren#2",
    "status": "done",
    "success": true,
    "error": null,
    "round": 0,
    "started_at": "2026-10-20T14:06:00Z",
    "finished_at": "2026-10-20T14:13:00Z",
    "tokens": {
     "input": 120000,
     "output": 3300
    },
    "output": "Found 4 undocumented endpoints in the bundle."
   },
   {
    "handle": "gannet#1",
    "kind": "workflow",
    "name": "auth-sweep (generated)",
    "run_id": "run_gannet#1",
    "parent": "lead",
    "depth": 1,
    "codename": "cobalt-harbor/heron>gannet#1",
    "address": "gannet#1",
    "status": "done",
    "success": true,
    "error": null,
    "round": 1,
    "started_at": "2026-10-20T14:26:00Z",
    "finished_at": "2026-10-20T15:23:00Z",
    "tokens": {
     "input": 2900000,
     "output": 61000
    },
    "output": "4 findings: reset-token reuse (high), missing invite rate limit (medium), role escalation via /admin/roles (critical), verbose session errors (low).",
    "workflow": {
     "name": "auth-sweep (generated)",
     "generated": true,
     "generated_by": "workflows.generate",
     "run_id": "run_auth-sweep",
     "codename": "cobalt-harbor/heron>gannet#1",
     "edges": [
      [
       "collect",
       "probe"
      ],
      [
       "probe",
       "review"
      ],
      [
       "review",
       "approve"
      ],
      [
       "approve",
       "report"
      ]
     ],
     "steps": [
      {
       "id": "collect",
       "kind": "step",
       "agent": "endpoint-collector",
       "state": "done",
       "start": 27,
       "end": 31
      },
      {
       "id": "probe",
       "kind": "for_each",
       "agent": "auth-prober",
       "state": "done",
       "start": 31,
       "end": 58,
       "units": [
        {
         "key": "/api/login",
         "state": "done"
        },
        {
         "key": "/api/logout",
         "state": "done"
        },
        {
         "key": "/api/reset",
         "state": "done"
        },
        {
         "key": "/api/invite",
         "state": "done"
        },
        {
         "key": "/api/session",
         "state": "done"
        },
        {
         "key": "/api/tokens",
         "state": "failed"
        },
        {
         "key": "/api/admin/users",
         "state": "done"
        },
        {
         "key": "/api/admin/roles",
         "state": "done"
        },
        {
         "key": "/api/export",
         "state": "done"
        },
        {
         "key": "/api/orders",
         "state": "done"
        },
        {
         "key": "/api/cart",
         "state": "done"
        },
        {
         "key": "/api/webhooks",
         "state": "done"
        }
       ]
      },
      {
       "id": "review",
       "kind": "panel",
       "state": "done",
       "start": 58,
       "end": 71,
       "panelists": [
        {
         "agent": "auth-reviewer",
         "verdict": "confirm"
        },
        {
         "agent": "session-reviewer",
         "verdict": "confirm"
        },
        {
         "agent": "skeptic",
         "verdict": "needs-evidence"
        }
       ],
       "gate": {
        "max_iterations": 2,
        "iterations": 2,
        "fixed_with": "auth-prober"
       }
      },
      {
       "id": "approve",
       "kind": "gate",
       "state": "done",
       "start": 71,
       "end": 76,
       "decision": "approved",
       "by": "operator"
      },
      {
       "id": "report",
       "kind": "step",
       "agent": "finding-writer",
       "state": "done",
       "start": 76,
       "end": 83
      }
     ]
    }
   },
   {
    "handle": "kite#1",
    "kind": "agent",
    "name": "exploit-verifier",
    "run_id": "run_kite#1",
    "parent": "lead",
    "depth": 1,
    "codename": "cobalt-harbor/heron>kite#1",
    "address": "kite#1",
    "status": "done",
    "success": true,
    "error": null,
    "round": 2,
    "started_at": "2026-10-20T15:26:00Z",
    "finished_at": "2026-10-20T15:44:00Z",
    "tokens": {
     "input": 780000,
     "output": 14000
    },
    "output": "Role escalation confirmed with a PoC request; screenshot attached."
   },
   {
    "handle": "kite#2",
    "kind": "agent",
    "name": "exploit-verifier",
    "run_id": "run_kite#2",
    "parent": "lead",
    "depth": 1,
    "codename": "cobalt-harbor/heron>kite#2",
    "address": "kite#2",
    "status": "failed",
    "success": false,
    "error": "provider overloaded (529) after 3 retries",
    "round": 2,
    "started_at": "2026-10-20T15:26:00Z",
    "finished_at": "2026-10-20T15:32:00Z",
    "tokens": {
     "input": 150000,
     "output": 2200
    },
    "output": null
   },
   {
    "handle": "kite#3",
    "kind": "agent",
    "name": "exploit-verifier",
    "run_id": "run_kite#3",
    "parent": "lead",
    "depth": 1,
    "codename": "cobalt-harbor/heron>kite#3",
    "address": "kite#3",
    "status": "done",
    "success": true,
    "error": null,
    "round": 2,
    "started_at": "2026-10-20T15:33:00Z",
    "finished_at": "2026-10-20T15:50:00Z",
    "tokens": {
     "input": 640000,
     "output": 11500
    },
    "output": "Reset-token reuse confirmed; tokens valid 24h after use."
   },
   {
    "handle": "kite#3>wren#3",
    "kind": "subagent",
    "name": "token-replayer",
    "run_id": "run_kite#3>wren#3",
    "parent": "kite#3",
    "depth": 2,
    "codename": "cobalt-harbor/heron>kite#3>wren#3",
    "address": "kite#3>wren#3",
    "status": "done",
    "success": true,
    "error": null,
    "round": 2,
    "started_at": "2026-10-20T15:37:00Z",
    "finished_at": "2026-10-20T15:44:00Z",
    "tokens": {
     "input": 70000,
     "output": 1900
    },
    "output": "Replayed 5 tokens, 5 accepted."
   },
   {
    "handle": "owl#1",
    "kind": "agent",
    "name": "engagement-reporter",
    "run_id": "run_owl#1",
    "parent": "lead",
    "depth": 1,
    "codename": "cobalt-harbor/heron>owl#1",
    "address": "owl#1",
    "status": "running",
    "success": null,
    "error": null,
    "round": 3,
    "started_at": "2026-10-20T15:54:00Z",
    "finished_at": null,
    "tokens": {
     "input": 310000,
     "output": 5200
    },
    "output": null
   }
  ],
  "messages": [
   {
    "id": "msg_001",
    "ts": "2026-10-20T14:02:00Z",
    "from": "lead",
    "to": "broadcast",
    "kind": "directive",
    "body": "Stay inside acme-shop staging. No destructive requests.",
    "urgent": false,
    "delivery": {
     "resolved": [
      "lynx#1",
      "lynx#2",
      "lynx#3"
     ],
     "undelivered": []
    }
   },
   {
    "id": "msg_002",
    "ts": "2026-10-20T14:09:00Z",
    "from": "lynx#1",
    "to": null,
    "kind": "post",
    "body": "claim: route map (/api, /admin, /auth)",
    "urgent": false,
    "delivery": {
     "resolved": [],
     "undelivered": []
    }
   },
   {
    "id": "msg_003",
    "ts": "2026-10-20T14:12:00Z",
    "from": "lynx#2",
    "to": null,
    "kind": "post",
    "body": "session lib 2.1.0 has advisory GHSA-xxxx (token fixation)",
    "urgent": false,
    "delivery": {
     "resolved": [],
     "undelivered": []
    }
   },
   {
    "id": "msg_004",
    "ts": "2026-10-20T14:14:00Z",
    "from": "lynx#2",
    "to": "lynx#1",
    "kind": "direct",
    "body": "Does /auth/reset use the session lib? Worth a look.",
    "urgent": false,
    "delivery": {
     "resolved": [
      "lynx#1"
     ],
     "undelivered": []
    }
   },
   {
    "id": "msg_005",
    "ts": "2026-10-20T14:16:00Z",
    "from": "lynx#1",
    "to": "lynx#2",
    "kind": "direct",
    "body": "Yes, reset tokens go through it. Flagging for round 1.",
    "urgent": false,
    "delivery": {
     "resolved": [
      "lynx#2"
     ],
     "undelivered": []
    }
   },
   {
    "id": "msg_006",
    "ts": "2026-10-20T14:20:00Z",
    "from": "lynx#3",
    "to": "lead",
    "kind": "direct",
    "body": "CORS allows *.acme-shop.test with credentials. Probably not exploitable alone.",
    "urgent": false,
    "delivery": {
     "resolved": [
      "lead"
     ],
     "undelivered": []
    }
   },
   {
    "id": "msg_007",
    "ts": "2026-10-20T14:25:00Z",
    "from": "operator",
    "to": "lead",
    "kind": "operator",
    "body": "Prioritise the admin roles endpoint, the client asked about it.",
    "urgent": true,
    "delivery": {
     "resolved": [
      "lead"
     ],
     "undelivered": []
    }
   },
   {
    "id": "msg_008",
    "ts": "2026-10-20T14:34:00Z",
    "from": "gannet#1>probe#6",
    "to": null,
    "kind": "post",
    "body": "claim: /api/tokens",
    "urgent": false,
    "delivery": {
     "resolved": [],
     "undelivered": []
    }
   },
   {
    "id": "msg_009",
    "ts": "2026-10-20T14:41:00Z",
    "from": "gannet#1>probe#6",
    "to": "gannet#1>probe#5",
    "kind": "direct",
    "body": "Seeing 500s on /api/tokens, are you hitting it too?",
    "urgent": false,
    "delivery": {
     "resolved": [
      "gannet#1>probe#5"
     ],
     "undelivered": []
    }
   },
   {
    "id": "msg_010",
    "ts": "2026-10-20T14:43:00Z",
    "from": "gannet#1>probe#5",
    "to": "gannet#1>probe#6",
    "kind": "direct",
    "body": "No, /session is clean. Might be rate limiting.",
    "urgent": false,
    "delivery": {
     "resolved": [
      "gannet#1>probe#6"
     ],
     "undelivered": []
    }
   },
   {
    "id": "msg_011",
    "ts": "2026-10-20T15:00:00Z",
    "from": "gannet#1>review#3",
    "to": "gannet#1>probe#7",
    "kind": "direct",
    "body": "Need a request/response pair for the role escalation before I confirm.",
    "urgent": false,
    "delivery": {
     "resolved": [
      "gannet#1>probe#7"
     ],
     "undelivered": []
    }
   },
   {
    "id": "msg_012",
    "ts": "2026-10-20T15:06:00Z",
    "from": "gannet#1>probe#7",
    "to": "gannet#1>review#3",
    "kind": "direct",
    "body": "Attached: PATCH /admin/roles as a viewer returns 200 and the role sticks.",
    "urgent": false,
    "delivery": {
     "resolved": [
      "gannet#1>review#3"
     ],
     "undelivered": []
    }
   },
   {
    "id": "msg_013",
    "ts": "2026-10-20T15:28:00Z",
    "from": "lead",
    "to": "role:exploit-verifier",
    "kind": "directive",
    "body": "Verify escalation and reset-token reuse. Capture evidence, no data changes.",
    "urgent": false,
    "delivery": {
     "resolved": [
      "kite#1",
      "kite#2",
      "kite#3"
     ],
     "undelivered": []
    }
   },
   {
    "id": "msg_014",
    "ts": "2026-10-20T15:32:00Z",
    "from": "kite#2",
    "to": "lead",
    "kind": "direct",
    "body": "Provider returned 529 three times, giving up.",
    "urgent": false,
    "delivery": {
     "resolved": [
      "lead"
     ],
     "undelivered": []
    }
   },
   {
    "id": "msg_015",
    "ts": "2026-10-20T15:35:00Z",
    "from": "lead",
    "to": "kite#3",
    "kind": "direct",
    "body": "Take reset-token reuse, kite#2 dropped it.",
    "urgent": false,
    "delivery": {
     "resolved": [
      "kite#3"
     ],
     "undelivered": []
    }
   },
   {
    "id": "msg_016",
    "ts": "2026-10-20T15:45:00Z",
    "from": "kite#1",
    "to": null,
    "kind": "post",
    "body": "VERIFIED: role escalation, PoC in evidence",
    "urgent": false,
    "delivery": {
     "resolved": [],
     "undelivered": []
    }
   },
   {
    "id": "msg_017",
    "ts": "2026-10-20T15:48:00Z",
    "from": "kite#3",
    "to": "owl#1",
    "kind": "direct",
    "body": "Evidence for reset reuse is in kite#3/evidence/",
    "urgent": false,
    "delivery": {
     "resolved": [],
     "undelivered": [
      {
       "to": "owl#1",
       "reason": "unknown_participant"
      }
     ]
    }
   },
   {
    "id": "msg_018",
    "ts": "2026-10-20T15:55:00Z",
    "from": "owl#1",
    "to": "kite#3",
    "kind": "direct",
    "body": "Picked up your evidence from the board, thanks.",
    "urgent": false,
    "delivery": {
     "resolved": [
      "kite#3"
     ],
     "undelivered": []
    }
   }
  ],
  "board": {
   "claims": [
    {
     "by": "lynx#1",
     "what": "route map",
     "at": "2026-10-20T14:09:00Z",
     "released": "2026-10-20T14:19:00Z"
    },
    {
     "by": "gannet#1>probe#6",
     "what": "/api/tokens",
     "at": "2026-10-20T14:34:00Z",
     "released": "2026-10-20T14:56:00Z"
    },
    {
     "by": "kite#3",
     "what": "reset-token reuse",
     "at": "2026-10-20T15:35:00Z",
     "released": "2026-10-20T15:50:00Z"
    },
    {
     "by": "owl#1",
     "what": "engagement report",
     "at": "2026-10-20T15:54:00Z",
     "released": null
    }
   ],
   "directives": [
    {
     "id": "msg_001",
     "ts": "2026-10-20T14:02:00Z",
     "from": "lead",
     "to": "broadcast",
     "kind": "directive",
     "body": "Stay inside acme-shop staging. No destructive requests.",
     "urgent": false,
     "delivery": {
      "resolved": [
       "lynx#1",
       "lynx#2",
       "lynx#3"
      ],
      "undelivered": []
     }
    },
    {
     "id": "msg_013",
     "ts": "2026-10-20T15:28:00Z",
     "from": "lead",
     "to": "role:exploit-verifier",
     "kind": "directive",
     "body": "Verify escalation and reset-token reuse. Capture evidence, no data changes.",
     "urgent": false,
     "delivery": {
      "resolved": [
       "kite#1",
       "kite#2",
       "kite#3"
      ],
      "undelivered": []
     }
    }
   ]
  },
  "participants": [
   {
    "address": "lead",
    "codename": "cobalt-harbor/heron",
    "agent": "audit-lead",
    "live": true
   },
   {
    "address": "lynx#1",
    "codename": "cobalt-harbor/heron>lynx#1",
    "agent": "surface-mapper",
    "live": false
   },
   {
    "address": "lynx#2",
    "codename": "cobalt-harbor/heron>lynx#2",
    "agent": "dependency-auditor",
    "live": false
   },
   {
    "address": "lynx#3",
    "codename": "cobalt-harbor/heron>lynx#3",
    "agent": "config-reviewer",
    "live": false
   },
   {
    "address": "lynx#1>wren#1",
    "codename": "cobalt-harbor/heron>lynx#1>wren#1",
    "agent": "route-crawler",
    "live": false
   },
   {
    "address": "lynx#1>wren#2",
    "codename": "cobalt-harbor/heron>lynx#1>wren#2",
    "agent": "js-bundle-reader",
    "live": false
   },
   {
    "address": "gannet#1",
    "codename": "cobalt-harbor/heron>gannet#1",
    "agent": "auth-sweep (generated)",
    "live": false
   },
   {
    "address": "kite#1",
    "codename": "cobalt-harbor/heron>kite#1",
    "agent": "exploit-verifier",
    "live": false
   },
   {
    "address": "kite#2",
    "codename": "cobalt-harbor/heron>kite#2",
    "agent": "exploit-verifier",
    "live": false
   },
   {
    "address": "kite#3",
    "codename": "cobalt-harbor/heron>kite#3",
    "agent": "exploit-verifier",
    "live": false
   },
   {
    "address": "kite#3>wren#3",
    "codename": "cobalt-harbor/heron>kite#3>wren#3",
    "agent": "token-replayer",
    "live": false
   },
   {
    "address": "owl#1",
    "codename": "cobalt-harbor/heron>owl#1",
    "agent": "engagement-reporter",
    "live": true
   }
  ],
  "now": "2026-10-20T15:58:00Z"
 },
 "workflowRun": {
  "run_id": "run_01M5MOCKWF",
  "name": "nightly-dependency-review",
  "codename": "amber-quay",
  "status": "completed",
  "started_at": "2026-10-20T14:00:00Z",
  "finished_at": "2026-10-20T14:42:00Z",
  "steps": [
   {
    "id": "inventory",
    "kind": "step",
    "agent": "dep-inventory",
    "state": "done",
    "start": 0,
    "end": 6
   },
   {
    "id": "triage",
    "kind": "parallel",
    "state": "done",
    "start": 6,
    "end": 24,
    "members": [
     {
      "agent": "npm-auditor",
      "state": "done"
     },
     {
      "agent": "cargo-auditor",
      "state": "done"
     },
     {
      "agent": "pip-auditor",
      "state": "done"
     }
    ]
   },
   {
    "id": "deep-dive",
    "kind": "for_each",
    "agent": "advisory-reader",
    "state": "done",
    "start": 24,
    "end": 36,
    "units": [
     {
      "key": "tokio",
      "state": "done"
     },
     {
      "key": "openssl",
      "state": "done"
     },
     {
      "key": "serde_yaml",
      "state": "done"
     },
     {
      "key": "lodash",
      "state": "done"
     },
     {
      "key": "requests",
      "state": "done"
     }
    ]
   },
   {
    "id": "summary",
    "kind": "step",
    "agent": "summary-writer",
    "state": "done",
    "start": 36,
    "end": 42
   }
  ],
  "edges": [
   [
    "inventory",
    "triage"
   ],
   [
    "triage",
    "deep-dive"
   ],
   [
    "deep-dive",
    "summary"
   ]
  ],
  "messages": [
   {
    "id": "m1",
    "ts": "2026-10-20T14:08:00Z",
    "from": "triage/npm-auditor",
    "to": null,
    "kind": "post",
    "body": "claim: package-lock.json"
   },
   {
    "id": "m2",
    "ts": "2026-10-20T14:10:00Z",
    "from": "triage/cargo-auditor",
    "to": "triage/pip-auditor",
    "kind": "direct",
    "body": "openssl shows up in both trees, I will take it."
   },
   {
    "id": "m3",
    "ts": "2026-10-20T14:11:00Z",
    "from": "triage/pip-auditor",
    "to": "triage/cargo-auditor",
    "kind": "direct",
    "body": "Ok, skipping pyOpenSSL then."
   },
   {
    "id": "m4",
    "ts": "2026-10-20T14:19:00Z",
    "from": "operator",
    "to": "broadcast",
    "kind": "operator",
    "body": "Ignore dev-only dependencies tonight.",
    "urgent": false
   },
   {
    "id": "m5",
    "ts": "2026-10-20T14:26:00Z",
    "from": "deep-dive#2",
    "to": "deep-dive#3",
    "kind": "direct",
    "body": "serde_yaml is unmaintained, not a CVE; note it, no severity."
   },
   {
    "id": "m6",
    "ts": "2026-10-20T14:37:00Z",
    "from": "summary/summary-writer",
    "to": null,
    "kind": "post",
    "body": "Draft summary ready: 2 upgrades, 1 replacement."
   }
  ]
 }
};
