select name, rank() over (order by age) from compat-sql
