select city, count(*) as c from compat-sql group by city having c > 1
