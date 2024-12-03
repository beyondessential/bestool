CREATE TABLE public.jobs (
    id integer GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    error text,
    topic text NOT NULL
);

COPY public.jobs (id, created_at, error, topic) FROM stdin;
1	1970-01-01 00:00:00 z	\N	bar
2	1970-01-01 00:00:00 z	err	foo
3	1970-01-01 00:00:00 z	err	bar
4	1970-01-01 00:00:00 z	err	baz
5	1970-01-01 00:00:00 z	err	qux
6	1970-01-01 00:00:00 z	err	foo
\.

SELECT pg_catalog.setval('public.jobs_id_seq', 6, true);
